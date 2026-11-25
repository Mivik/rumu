//! The external plugins of `settings.yaml`'s `plugins:`, each a
//! [`StdioPlugin`] process with a status. One plugin failing to start, or
//! exiting later, takes only itself out: it shows as failed and can be
//! restarted. Editing the list (from the web) restarts the entries that
//! changed, starts the new ones and stops the removed ones, without
//! restarting the server. Whoever owns the kernel follows
//! [`ExternalPlugins::subscribe`] and hands [`ExternalPlugins::plugins`] to
//! it.
//!
//! A plugin may serve a top-level directory of `@rumu` (its `mount`):
//! [`ExternalPlugins::mounts`] hands the workspace plugin the running ones.
//! A root that is not one plain segment, that rumu uses itself, or that a
//! plugin before it in name order serves, is left out with a warning.
//!
//! A plugin restarts by itself when a file its command names changes (its
//! built backend, a script), and its web entry gets a new URL when that file
//! changes: `vite build --watch` reloads a plugin under development.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rumu_core::{BranchId, Plugin, PluginInfo, PluginMount, PluginSettings, PluginStatus};
use serde_json::{Map, Value};
use tokio::sync::watch;
use tracing::{info, warn};

use crate::StdioPlugin;
use crate::stdio::StdioError;

/// How often the files a plugin is built into are checked for changes.
const BUILD_POLL: Duration = Duration::from_millis(500);
/// A changed file this young may be half written: it waits a poll.
const BUILD_SETTLE: Duration = Duration::from_millis(300);

type Warn = Arc<dyn Fn(String) + Send + Sync>;

/// An entry of `plugins:`, resolved against `RUMU_HOME`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Spec {
    name: String,
    argv: Vec<String>,
    /// As written (shown in the web).
    cwd: Option<String>,
    dir: PathBuf,
}

/// Every external plugin, in name order (the order their hooks register
/// in).
pub struct ExternalPlugins {
    home: PathBuf,
    entries: Mutex<Vec<Entry>>,
    changed: watch::Sender<u64>,
    next_start: AtomicU64,
    warn: OnceLock<Warn>,
}

struct Entry {
    spec: Spec,
    /// Its settings values.
    settings: Map<String, Value>,
    state: State,
    /// The files it is built into, as last seen.
    watched: Vec<Watched>,
}

/// A file a plugin restarts on (`web`: whose URL changes) when it changes.
struct Watched {
    path: PathBuf,
    web: bool,
    stamp: Option<SystemTime>,
}

fn stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Spec {
    /// The files its command names (past the program), as they are now.
    fn watched(&self) -> Vec<Watched> {
        self.argv
            .iter()
            .skip(1)
            .map(|a| self.dir.join(a))
            .filter(|p| p.is_file())
            .map(|path| Watched {
                stamp: stamp(&path),
                path,
                web: false,
            })
            .collect()
    }
}

enum State {
    /// Start number `n` is in flight; a start finding another number was
    /// superseded and drops its process.
    Starting(u64),
    Running(Arc<StdioPlugin>),
    Failed(String),
}

impl ExternalPlugins {
    pub fn new(home: &Path) -> Arc<Self> {
        Arc::new(Self {
            home: home.to_owned(),
            entries: Mutex::default(),
            changed: watch::channel(0).0,
            next_start: AtomicU64::new(1),
            warn: OnceLock::new(),
        })
    }

    /// Ticks whenever a plugin starts, stops or fails.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Where problems go besides the log (the clients' warnings). Set once.
    pub fn on_warning(&self, f: impl Fn(String) + Send + Sync + 'static) {
        let _ = self.warn.set(Arc::new(f));
    }

    fn warn(&self, message: String) {
        warn!("{message}");
        if let Some(f) = self.warn.get() {
            f(message);
        }
    }

    fn notify(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().expect("plugin entries lock")
    }

    /// Match the plugins to `plugins:`: unchanged running ones stay,
    /// removed ones stop, and new, changed or failed ones (re)start in the
    /// background ([`Self::settled`] waits for them).
    pub fn configure(self: &Arc<Self>, plugins: &BTreeMap<String, PluginSettings>) {
        let starts = {
            let mut entries = self.lock();
            let mut old = std::mem::take(&mut *entries);
            let mut starts = Vec::new();
            for (name, p) in plugins {
                let spec = Spec {
                    name: name.clone(),
                    argv: p.command.clone(),
                    cwd: p.cwd.clone(),
                    dir: self.home.join(p.cwd.as_deref().unwrap_or(".")),
                };
                let kept = old
                    .iter()
                    .position(|e| e.spec == spec && !matches!(e.state, State::Failed(_)));
                match kept {
                    Some(i) => {
                        let mut e = old.swap_remove(i);
                        if e.settings != p.settings {
                            e.settings = p.settings.clone();
                            if let State::Running(plugin) = &e.state {
                                plugin.set_settings(&e.settings);
                            }
                        }
                        entries.push(e);
                    }
                    None => {
                        let n = self.next_start.fetch_add(1, Ordering::Relaxed);
                        starts.push((spec.clone(), p.settings.clone(), n));
                        entries.push(Entry {
                            spec,
                            settings: p.settings.clone(),
                            state: State::Starting(n),
                            watched: Vec::new(),
                        });
                    }
                }
            }
            for e in old {
                if let State::Running(p) = e.state {
                    p.shutdown();
                }
            }
            starts
        };
        self.notify();
        for (spec, settings, n) in starts {
            tokio::spawn(self.clone().start(spec, settings, n));
        }
    }

    /// Wait until no plugin is starting.
    pub async fn settled(&self) {
        let mut changed = self.subscribe();
        let starting = || {
            self.lock()
                .iter()
                .any(|e| matches!(e.state, State::Starting(_)))
        };
        while starting() {
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    /// Stop plugin `name` if it runs and start it again. False when no
    /// plugin has that name.
    pub fn restart(self: &Arc<Self>, name: &str) -> bool {
        let n = self.next_start.fetch_add(1, Ordering::Relaxed);
        let spec = {
            let mut entries = self.lock();
            let Some(e) = entries.iter_mut().find(|e| e.spec.name == name) else {
                return false;
            };
            if let State::Running(p) = &e.state {
                p.shutdown();
            }
            e.state = State::Starting(n);
            (e.spec.clone(), e.settings.clone())
        };
        self.notify();
        tokio::spawn(self.clone().start(spec.0, spec.1, n));
        true
    }

    /// Take plugin `name` out (the kernel could not register it): stop it
    /// and show `error`.
    pub fn reject(&self, name: &str, error: &str) {
        {
            let mut entries = self.lock();
            let Some(e) = entries
                .iter_mut()
                .find(|e| matches!(&e.state, State::Running(p) if p.manifest().name == name))
            else {
                return;
            };
            if let State::Running(p) = &e.state {
                p.shutdown();
            }
            e.state = State::Failed(error.to_owned());
        }
        self.warn(format!("plugin `{name}` was left out: {error}"));
        self.notify();
    }

    async fn start(self: Arc<Self>, spec: Spec, settings: Map<String, Value>, n: u64) {
        // Seen before the start, so a change while it starts restarts it.
        let mut watched = spec.watched();
        let started = StdioPlugin::spawn(&spec.name, &spec.argv, &spec.dir, &settings).await;
        {
            let mut entries = self.lock();
            // Superseded (removed, changed or restarted meanwhile): the
            // process is dropped, which stops it.
            let Some(e) = entries
                .iter_mut()
                .find(|e| matches!(e.state, State::Starting(m) if m == n))
            else {
                return;
            };
            match started {
                Ok(p) => {
                    let p = Arc::new(p);
                    if let Some(web) = p.extras().web {
                        let path = spec.dir.join(web);
                        watched.push(Watched {
                            stamp: stamp(&path),
                            path,
                            web: true,
                        });
                    }
                    e.watched = watched;
                    e.state = State::Running(p.clone());
                    let refused = p.extras().mount.and_then(|m| {
                        let why = refused_mount(&entries, &spec.name, &m)?;
                        Some(format!(
                            "plugin `{}` does not mount `@rumu:/{}`: {why}",
                            spec.name, m.root
                        ))
                    });
                    tokio::spawn(watch_exit(Arc::downgrade(&self), p));
                    if let Some(message) = refused {
                        drop(entries);
                        self.warn(message);
                    }
                }
                Err(err) => {
                    e.watched = watched;
                    e.state = State::Failed(err.to_string());
                    drop(entries);
                    self.warn(format!("plugin `{}` failed to start: {err}", spec.name));
                }
            }
        }
        self.notify();
    }

    /// The running plugins, in name order.
    pub fn plugins(&self) -> Vec<Arc<dyn Plugin>> {
        self.lock()
            .iter()
            .filter_map(|e| match &e.state {
                State::Running(p) => Some(p.clone() as Arc<dyn Plugin>),
                _ => None,
            })
            .collect()
    }

    /// The `@rumu` directories the running plugins serve, in name order,
    /// each with its plugin; refused ones are left out.
    pub fn mounts(&self) -> Vec<(PluginMount, Arc<StdioPlugin>)> {
        let entries = self.lock();
        entries
            .iter()
            .filter_map(|e| match &e.state {
                State::Running(p) => {
                    let m = p.extras().mount?;
                    refused_mount(&entries, &e.spec.name, &m)
                        .is_none()
                        .then(|| (m, p.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The web's view: each plugin with its status.
    pub fn info(&self) -> Vec<PluginInfo> {
        self.lock()
            .iter()
            .map(|e| {
                let (status, error, manifest) = match &e.state {
                    State::Starting(_) => (PluginStatus::Starting, None, None),
                    State::Running(p) => (PluginStatus::Running, None, Some(p.manifest())),
                    State::Failed(error) => (PluginStatus::Failed, Some(error.clone()), None),
                };
                let manifest = manifest.unwrap_or_default();
                let extras = match &e.state {
                    State::Running(p) => p.extras(),
                    _ => Default::default(),
                };
                // The URL changes with the file, so the page loads it anew.
                let web = e.watched.iter().find(|w| w.web).map(|w| {
                    let file = w.path.file_name().unwrap_or_default().to_string_lossy();
                    let v = w
                        .stamp
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis());
                    format!("/plugins/{}/{file}?v={v}", e.spec.name)
                });
                PluginInfo {
                    name: e.spec.name.clone(),
                    command: e.spec.argv.clone(),
                    cwd: e.spec.cwd.clone(),
                    status,
                    error,
                    tools: manifest.tools.into_iter().map(|t| t.name).collect(),
                    hooks: manifest.hooks.into_iter().map(|h| h.id).collect(),
                    web,
                    schema: extras.settings,
                    settings: e.settings.clone(),
                }
            })
            .collect()
    }
}

impl ExternalPlugins {
    /// Ask running plugin `name` (for its web side's `ctx.call`).
    pub async fn call(
        &self,
        name: &str,
        method: &str,
        params: Value,
        branch: Option<&BranchId>,
    ) -> Result<Value, StdioError> {
        let running = self.lock().iter().find_map(|e| match &e.state {
            State::Running(p) if e.spec.name == name => Some(p.clone()),
            _ => None,
        });
        let p = running.ok_or_else(|| StdioError::NotRunning {
            name: name.to_owned(),
        })?;
        p.call(method, params, branch).await
    }

    /// The directory plugin `name`'s web entry is in, while it runs: what
    /// `/plugins/<name>/` serves.
    pub fn web_dir(&self, name: &str) -> Option<PathBuf> {
        self.lock()
            .iter()
            .filter(|e| e.spec.name == name && matches!(e.state, State::Running(_)))
            .find_map(|e| e.watched.iter().find(|w| w.web))
            .and_then(|w| w.path.parent().map(Path::to_owned))
    }

    /// Follow the files plugins are built into: a changed backend restarts
    /// its plugin, a changed web entry gets a new URL.
    pub fn watch_builds(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(BUILD_POLL).await;
                let Some(this) = weak.upgrade() else { return };
                let (mut restart, mut web) = (Vec::new(), false);
                for e in this.lock().iter_mut() {
                    if matches!(e.state, State::Starting(_)) {
                        continue;
                    }
                    for w in &mut e.watched {
                        let now = stamp(&w.path);
                        let settled = now
                            .and_then(|t| t.elapsed().ok())
                            .is_none_or(|age| age >= BUILD_SETTLE);
                        if now == w.stamp || !settled {
                            continue;
                        }
                        w.stamp = now;
                        if w.web {
                            web = true;
                        } else if !restart.contains(&e.spec.name) {
                            restart.push(e.spec.name.clone());
                        }
                    }
                }
                for name in restart {
                    info!(plugin = %name, "rebuilt; restarting");
                    this.restart(&name);
                }
                if web {
                    this.notify();
                }
            }
        });
    }
}

/// Why plugin `name` may not serve `mount`, if it may not: its root is not
/// one plain segment, rumu has a directory by that name, or a running plugin
/// before it in name order serves it.
fn refused_mount(entries: &[Entry], name: &str, mount: &PluginMount) -> Option<String> {
    let root = mount.root.as_str();
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if root.is_empty() || !root.chars().all(plain) {
        return Some("a root is one segment of letters, digits, `-` and `_`".into());
    }
    if crate::workspace::vfs::builtin_roots().contains(&root) {
        return Some("rumu has a directory by that name".into());
    }
    entries
        .iter()
        .take_while(|e| e.spec.name != name)
        .find(|e| match &e.state {
            State::Running(p) => p.extras().mount.is_some_and(|m| m.root == root),
            _ => false,
        })
        .map(|e| format!("plugin `{}` serves it", e.spec.name))
}

/// Mark `p` failed when its process ends on its own (a stop the host asked
/// for has already replaced it in its entry).
async fn watch_exit(hub: Weak<ExternalPlugins>, p: Arc<StdioPlugin>) {
    let how = p.exited().await;
    let Some(hub) = hub.upgrade() else { return };
    let name = {
        let mut entries = hub.lock();
        let Some(e) = entries
            .iter_mut()
            .find(|e| matches!(&e.state, State::Running(q) if Arc::ptr_eq(q, &p)))
        else {
            return;
        };
        e.state = State::Failed(how.clone());
        e.spec.name.clone()
    };
    hub.warn(format!("plugin `{name}` {how}"));
    hub.notify();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeRuntime;
    use serde_json::json;
    use std::time::Duration;

    /// A plugin with one tool, `crash`, that writes to stderr and exits 3.
    const CRASH: &str = r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

for line in sys.stdin.buffer:
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"name": "", "tools": [{
            "name": "crash", "description": "exits",
            "schema": {"type": "object", "properties": {}},
        }]}})
    elif m == "tool/call":
        print("boom", file=sys.stderr, flush=True)
        sys.exit(3)
"#;

    fn home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("crash.py"), CRASH).unwrap();
        dir
    }

    /// A `plugins:` list of `(name, command)`.
    fn plugins(list: &[(&str, &[&str])]) -> BTreeMap<String, PluginSettings> {
        list.iter()
            .map(|(name, cmd)| {
                let command = cmd.iter().map(|s| s.to_string()).collect();
                let p = PluginSettings {
                    command,
                    ..Default::default()
                };
                (name.to_string(), p)
            })
            .collect()
    }

    fn statuses(hub: &ExternalPlugins) -> Vec<(String, PluginStatus)> {
        hub.info().into_iter().map(|i| (i.name, i.status)).collect()
    }

    /// Wait until plugin `name` has `status`.
    async fn until(hub: &ExternalPlugins, name: &str, status: PluginStatus) -> PluginInfo {
        let mut changed = hub.subscribe();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(i) = hub.info().into_iter().find(|i| i.name == name)
                    && i.status == status
                {
                    return i;
                }
                changed.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }

    /// A plugin that cannot start fails alone; one that exits mid-call
    /// fails the call at once, shows how it ended, and restarts.
    #[tokio::test]
    async fn failures_stay_local_and_exits_are_seen() {
        let dir = home();
        let hub = ExternalPlugins::new(dir.path());
        hub.configure(&plugins(&[
            ("crash", &["python3", "crash.py"]),
            ("missing", &["/nonexistent/plugin"]),
            ("nothing", &[]),
        ]));
        hub.settled().await;
        use PluginStatus::*;
        let names = |v: &[(&str, PluginStatus)]| {
            v.iter()
                .map(|(n, s)| (n.to_string(), *s))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            statuses(&hub),
            names(&[("crash", Running), ("missing", Failed), ("nothing", Failed)])
        );
        let info = hub.info();
        assert!(info[1].error.as_ref().unwrap().contains("spawning"));
        assert_eq!(info[0].tools, ["crash"]);

        let plugins = hub.plugins();
        assert_eq!(plugins.len(), 1);
        let cx = FakeRuntime::arc().cx();
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            plugins[0].call_tool("crash", json!({}), &cx),
        )
        .await
        .expect("a call to an exited plugin fails at once")
        .unwrap();
        assert!(out.is_error);
        let failed = until(&hub, "crash", Failed).await;
        let error = failed.error.unwrap();
        assert!(
            error.contains("status 3") && error.contains("boom"),
            "{error}"
        );
        assert!(hub.plugins().is_empty());

        assert!(hub.restart("crash"));
        until(&hub, "crash", Running).await;
        assert!(!hub.restart("nope"));
    }

    /// Editing the list keeps the unchanged plugins, stops the removed ones
    /// and starts the new ones.
    #[tokio::test]
    async fn configure_applies_only_what_changed() {
        let dir = home();
        let crash: &[&str] = &["python3", "crash.py"];
        let hub = ExternalPlugins::new(dir.path());
        hub.configure(&plugins(&[("a", crash), ("b", crash)]));
        hub.settled().await;
        let before = hub.plugins();
        assert_eq!(before.len(), 2);

        hub.configure(&plugins(&[("a", crash), ("c", crash)]));
        hub.settled().await;
        let after = hub.plugins();
        let names: Vec<String> = hub.info().into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["a", "c"]);
        assert!(Arc::ptr_eq(&before[0], &after[0]), "a kept running");
        assert!(!Arc::ptr_eq(&before[1], &after[1]));
    }
}
