//! `StdioPlugin` — host side of the external plugin protocol.
//!
//! A child process speaking newline-delimited JSON-RPC 2.0 over stdin/stdout.
//! Host→plugin requests (`initialize`, `hook`, `tool/call`, and `call` from
//! the plugin's web side) are correlated via a pending-map of oneshot
//! channels. They time out after 30s, except `tool/call` and the `vfs/*`
//! requests, which have no transport timeout — they run inside a tool call,
//! which the kernel cancels by dropping the request future (abort /
//! `task.kill`), so a tool may run as long as it needs. The `settings`
//! notification hands over edited settings values.
//! Plugin→host requests (`host/path`, `host/state`, `host/index`, `host/post`,
//! `host/progress`, `host/emit`) arrive on the same stdout stream *while* a
//! host→plugin request is in flight, so the reader task dispatches them on
//! spawned tasks of their own.
//!
//! `initialize` carries rumu's `version` and the plugin's settings values;
//! besides the [`Manifest`], its result may name the rumu versions the plugin
//! needs (`rumu`, a semver requirement: unmet, the start fails), its web
//! entry (`web`), the JSON Schema of its settings (`settings`) and the
//! `@rumu` directory it serves (`mount`, a [`PluginMount`]).
//!
//! A mount's paths are served by `vfs/list`, `vfs/read`, `vfs/write`,
//! `vfs/delete`, `vfs/search` and `vfs/invoke` (see [`StdioPlugin::vfs`]),
//! each with the path below the root (`/` for the root itself).
//!
//! Every kernel→plugin request carries a `ctx` token. The plugin echoes that
//! token in its `host/...` requests; the host serves them from the [`Cx`] kept
//! for that token while the request is in flight. (A bare "most recent Cx"
//! would serve concurrent requests — e.g. parallel sub-agents — against the
//! wrong branch.) `host/post` needs no token: it targets an explicit branch
//! and its source is this plugin's name. `host/progress` reports live output
//! for a running tool call (`{ ctx, append?, data? }`). `host/emit`
//! (`{ event, data?, branch? | ctx? }`) pushes an event to the plugin's web
//! side, on the clients showing that branch's session (all without one).
//!
//! When the process exits, every request in flight fails at once and
//! [`StdioPlugin::exited`] resolves with how it ended (status and the tail of
//! its stderr); [`crate::ExternalPlugins`] takes it from there.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    process::Stdio,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use rumu_core::{
    Cx, Event, HookOutput, Inbox, IoError, Manifest, Plugin, PluginMount, Post, Progress,
    ToolOutput,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{mpsc, oneshot, watch},
};
use tracing::{info, warn};

use crate::Result;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The rumu version `initialize` tells plugins, and their `rumu`
/// requirement is checked against.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Stderr lines kept for the message of a failed start or an exit.
const STDERR_TAIL: usize = 20;

/// A failure of the external-plugin transport or of its host-service
/// protocol.
#[derive(Debug, thiserror::Error)]
pub enum StdioError {
    /// A filesystem operation on the plugin config failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// A request or response could not be serialized.
    #[error("serializing a plugin message: {0}")]
    Json(#[from] serde_json::Error),

    /// The plugin entry has no command.
    #[error("empty argv for stdio plugin `{name}`")]
    EmptyArgv { name: String },

    /// The child process could not be started.
    #[error("spawning stdio plugin `{name}` (argv {argv:?}): {source}")]
    Spawn {
        name: String,
        argv: Vec<String>,
        #[source]
        source: std::io::Error,
    },

    /// One of the child's stdio pipes was not available.
    #[error("child {stream} not piped")]
    NotPiped { stream: &'static str },

    /// The `initialize` handshake failed; `stderr` is the tail of what the
    /// process wrote there.
    #[error("initializing stdio plugin `{name}`: {source}{stderr}")]
    Initialize {
        name: String,
        #[source]
        source: Box<StdioError>,
        stderr: String,
    },

    /// The plugin needs another rumu version.
    #[error("plugin `{name}` needs rumu {wants}, this is {VERSION}")]
    Version { name: String, wants: String },

    /// The plugin's manifest was not valid JSON.
    #[error("invalid manifest from stdio plugin `{name}`: {source}")]
    InvalidManifest {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    /// The plugin's writer task is gone.
    #[error("plugin `{name}` is not running (writer closed)")]
    NotRunning { name: String },

    /// A request timed out.
    #[error("plugin `{name}` timed out after {timeout:?} on `{method}`")]
    Timeout {
        name: String,
        method: String,
        timeout: Duration,
    },

    /// The plugin closed its stdout (it exited) before answering.
    #[error("plugin `{name}` exited before answering `{method}`")]
    Dropped { name: String, method: String },

    /// The plugin answered with a JSON-RPC error.
    #[error("plugin `{name}` error on `{method}`: {message}")]
    Remote {
        name: String,
        method: String,
        message: String,
    },

    /// `host/post` is missing its branch.
    #[error("host/post is missing `branch`")]
    MissingBranch,

    /// A host-service payload was invalid.
    #[error("invalid `{what}` payload: {source}")]
    InvalidPayload {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// No inbox was attached yet.
    #[error("the kernel has not attached an inbox yet")]
    NoInbox,

    /// A host request is missing its `ctx` token.
    #[error("host request is missing its `ctx` token")]
    MissingCtx,

    /// The `ctx` token is unknown or expired.
    #[error("unknown or expired ctx token `{token}`")]
    UnknownCtx { token: String },

    /// `host/progress` data was invalid.
    #[error("invalid `host/progress` data: {source}")]
    InvalidProgress {
        #[source]
        source: serde_json::Error,
    },

    /// `host/progress` carries neither text nor data.
    #[error("host/progress needs `append` or `data`")]
    ProgressNeedsArg,

    /// `host/index` is missing its tag.
    #[error("host/index is missing `tag`")]
    MissingTag,

    /// `host/emit` is missing its event name.
    #[error("host/emit is missing `event`")]
    MissingEvent,

    /// A host method is not implemented.
    #[error("unknown host method `{method}`")]
    UnknownHostMethod { method: String },
}

type StdioResult<T> = std::result::Result<T, StdioError>;

/// What `initialize` answers besides the [`Manifest`].
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct Extras {
    #[serde(default)]
    pub rumu: Option<String>,
    #[serde(default)]
    pub web: Option<String>,
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub mount: Option<PluginMount>,
}

struct Inner {
    name: String,
    manifest: RwLock<Manifest>,
    extras: RwLock<Extras>,
    /// Serialized outbound lines (requests + responses) written by one task.
    writer: mpsc::Sender<String>,
    pending: Mutex<HashMap<String, oneshot::Sender<std::result::Result<Value, String>>>>,
    next_id: AtomicU64,
    next_ctx: AtomicU64,
    /// Per-request contexts, keyed by the `ctx` token echoed back by the plugin.
    ctxs: Mutex<HashMap<String, Cx>>,
    /// The kernel's out-of-band channel, bound to this plugin's name.
    inbox: Mutex<Option<Arc<dyn Inbox>>>,
    /// The last lines of the process's stderr.
    stderr: Mutex<VecDeque<String>>,
    /// Asks the waiter task to kill the process.
    kill: Mutex<Option<oneshot::Sender<()>>>,
    /// How the process ended, once it has.
    exit: watch::Sender<Option<String>>,
}

/// A spawned external plugin. `initialize` is performed in [`StdioPlugin::spawn`]
/// and the manifest cached for [`Plugin::manifest`].
pub struct StdioPlugin {
    inner: Arc<Inner>,
}

// ---------------------------------------------------------------------------
// spawn + tasks
// ---------------------------------------------------------------------------

impl StdioPlugin {
    /// Spawn the child, start the I/O tasks and perform the `initialize`
    /// handshake, handing it `settings`. `cwd` is the working directory for
    /// the child.
    pub async fn spawn(
        name: &str,
        argv: &[String],
        cwd: &Path,
        settings: &serde_json::Map<String, Value>,
    ) -> StdioResult<Self> {
        if argv.is_empty() {
            return Err(StdioError::EmptyArgv {
                name: name.to_owned(),
            });
        }
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| StdioError::Spawn {
                name: name.to_owned(),
                argv: argv.to_vec(),
                source,
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or(StdioError::NotPiped { stream: "stdin" })?;
        let stdout = child
            .stdout
            .take()
            .ok_or(StdioError::NotPiped { stream: "stdout" })?;
        let stderr = child
            .stderr
            .take()
            .ok_or(StdioError::NotPiped { stream: "stderr" })?;

        let (tx, rx) = mpsc::channel::<String>(64);
        let (kill, killed) = oneshot::channel();
        let inner = Arc::new(Inner {
            name: name.to_string(),
            manifest: RwLock::new(Manifest::default()),
            extras: RwLock::default(),
            writer: tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            next_ctx: AtomicU64::new(1),
            ctxs: Mutex::new(HashMap::new()),
            inbox: Mutex::new(None),
            stderr: Mutex::default(),
            kill: Mutex::new(Some(kill)),
            exit: watch::channel(None).0,
        });

        tokio::spawn(writer_task(stdin, rx));
        let stderr = tokio::spawn(stderr_task(inner.clone(), stderr));
        tokio::spawn(reader_task(inner.clone(), stdout));
        tokio::spawn(waiter_task(inner.clone(), child, killed, stderr));

        let initialized = request(
            &inner,
            "initialize",
            json!({ "host": "rumu", "version": VERSION, "settings": settings }),
        )
        .await;
        let result = match initialized {
            Ok(result) => result,
            Err(source) => {
                // A process that died on start says why on stderr: wait
                // briefly for it to finish writing.
                let mut exit = inner.exit.subscribe();
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), exit.wait_for(Option::is_some))
                        .await;
                kill_child(&inner);
                return Err(StdioError::Initialize {
                    name: name.to_owned(),
                    source: Box::new(source),
                    stderr: stderr_suffix(&inner),
                });
            }
        };
        let invalid = |source| StdioError::InvalidManifest {
            name: name.to_owned(),
            source,
        };
        let extras: Extras = serde_json::from_value(result.clone()).map_err(invalid)?;
        let mut manifest: Manifest = serde_json::from_value(result).map_err(invalid)?;
        if let Some(wants) = &extras.rumu {
            let meets = semver::VersionReq::parse(wants)
                .is_ok_and(|req| req.matches(&semver::Version::parse(VERSION).expect("semver")));
            if !meets {
                kill_child(&inner);
                return Err(StdioError::Version {
                    name: name.to_owned(),
                    wants: wants.clone(),
                });
            }
        }
        if manifest.name.trim().is_empty() {
            manifest.name = name.to_string();
        }
        *inner.manifest.write().unwrap() = manifest;
        *inner.extras.write().unwrap() = extras;
        Ok(Self { inner })
    }

    /// What `initialize` answered besides the manifest.
    pub fn extras(&self) -> Extras {
        self.inner.extras.read().unwrap().clone()
    }

    /// Ask the plugin's `call` handler (its web side's `ctx.call`), on
    /// behalf of a client showing `branch`.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        branch: Option<&rumu_core::BranchId>,
    ) -> StdioResult<Value> {
        let params = json!({ "method": method, "params": params, "branch": branch });
        request(&self.inner, "call", params).await
    }

    /// Ask the plugin about a path of its mount: `op` is `list`, `read`,
    /// `write`, `delete`, `search` or `invoke`, `params` an object with
    /// the path below the root (`path`) and what the op takes. The request
    /// carries the calling branch and a `ctx` token, like `tool/call`.
    pub async fn vfs(&self, op: &str, mut params: Value, cx: &Cx) -> StdioResult<Value> {
        let ctx = self.begin_ctx(cx);
        if let Some(obj) = params.as_object_mut() {
            obj.insert("ctx".into(), json!(ctx.token));
            obj.insert("session".into(), json!(cx.session));
            obj.insert("branch".into(), json!(cx.branch));
        }
        let result = request(&self.inner, &format!("vfs/{op}"), params).await;
        drop(ctx);
        result
    }

    /// Hand the plugin its edited settings values.
    pub fn set_settings(&self, settings: &serde_json::Map<String, Value>) {
        let msg = json!({ "jsonrpc": "2.0", "method": "settings", "params": settings });
        if let Ok(line) = serde_json::to_string(&msg) {
            let _ = self.inner.writer.try_send(line);
        }
    }

    /// Register `cx` for one in-flight request. The returned guard holds the
    /// `ctx` token and unregisters it on drop — including when the kernel
    /// drops the request future on abort.
    fn begin_ctx(&self, cx: &Cx) -> CtxGuard<'_> {
        let n = self.inner.next_ctx.fetch_add(1, Ordering::Relaxed);
        let token = format!("ctx{n}");
        self.inner
            .ctxs
            .lock()
            .unwrap()
            .insert(token.clone(), cx.clone());
        CtxGuard {
            inner: &self.inner,
            token,
        }
    }
}

struct CtxGuard<'a> {
    inner: &'a Inner,
    token: String,
}

impl Drop for CtxGuard<'_> {
    fn drop(&mut self) {
        self.inner.ctxs.lock().unwrap().remove(&self.token);
    }
}

impl StdioPlugin {
    /// Stop the process: a `shutdown` notification, then a kill. Requests
    /// in flight fail.
    pub fn shutdown(&self) {
        if let Ok(line) = serde_json::to_string(&json!({ "jsonrpc": "2.0", "method": "shutdown" }))
        {
            let _ = self.inner.writer.try_send(line);
        }
        kill_child(&self.inner);
    }

    /// Wait until the process has ended; how it ended, with the tail of its
    /// stderr.
    pub async fn exited(&self) -> String {
        let mut exit = self.inner.exit.subscribe();
        let ended = exit.wait_for(Option::is_some).await.map(|e| e.clone());
        match ended {
            Ok(Some(how)) => format!("{how}{}", stderr_suffix(&self.inner)),
            _ => "exited".into(),
        }
    }
}

impl Drop for StdioPlugin {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn kill_child(inner: &Inner) {
    if let Some(kill) = inner.kill.lock().unwrap().take() {
        let _ = kill.send(());
    }
}

/// The stderr tail as a message suffix: empty, or a newline and the lines.
fn stderr_suffix(inner: &Inner) -> String {
    let tail = inner.stderr.lock().unwrap();
    if tail.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
    format!("\n{}", lines.join("\n"))
}

/// Own the child: kill it when asked (or when the plugin is gone), and
/// record how it ended — after its last stderr lines are in (bounded: a
/// grandchild may hold the pipe open).
async fn waiter_task(
    inner: Arc<Inner>,
    mut child: Child,
    killed: oneshot::Receiver<()>,
    stderr: tokio::task::JoinHandle<()>,
) {
    let status = tokio::select! {
        status = child.wait() => status,
        _ = killed => {
            let _ = child.start_kill();
            child.wait().await
        }
    };
    let how = match status {
        Ok(s) => match s.code() {
            Some(code) => format!("exited with status {code}"),
            None => format!("ended by a signal ({s})"),
        },
        Err(e) => format!("could not be waited on: {e}"),
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), stderr).await;
    info!(plugin = %inner.name, "{how}");
    inner.exit.send_replace(Some(how));
}

async fn writer_task(mut stdin: ChildStdin, mut rx: mpsc::Receiver<String>) {
    while let Some(line) = rx.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() {
            break;
        }
        if stdin.write_all(b"\n").await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn stderr_task(inner: Arc<Inner>, stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        info!(plugin = %inner.name, "{line}");
        let mut tail = inner.stderr.lock().unwrap();
        if tail.len() == STDERR_TAIL {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

async fn reader_task(inner: Arc<Inner>, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let Ok(Some(line)) = lines.next_line().await else {
            break;
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(line) else {
            warn!(plugin = %inner.name, line = %line, "non-JSON stdout");
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            // Notification from the plugin (method only): nothing to answer.
            continue;
        };
        if msg.get("method").is_some() {
            // A plugin→host request; dispatch independently so it can be served
            // while our own request to the plugin is still in flight.
            tokio::spawn(handle_plugin_request(inner.clone(), id, msg));
        } else {
            // Response to one of our requests.
            let key = id_key(&id);
            let Some(tx) = inner.pending.lock().unwrap().remove(&key) else {
                continue;
            };
            let result = match msg.get("error") {
                Some(err) => Err(err
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| err.to_string())),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(result);
        }
    }
    // The process closed stdout: no answer is coming. Failing the requests
    // in flight now beats leaving a `tool/call` (no timeout) hanging.
    inner.pending.lock().unwrap().clear();
}

// ---------------------------------------------------------------------------
// host→plugin requests
// ---------------------------------------------------------------------------

/// Remove a request's pending entry when its future is dropped before the
/// response arrives — the kernel cancels a tool call by dropping the future,
/// so its entry would otherwise leak.
struct PendingGuard<'a> {
    inner: &'a Inner,
    id: String,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.inner.pending.lock().unwrap().remove(&self.id);
    }
}

async fn request(inner: &Inner, method: &str, params: Value) -> StdioResult<Value> {
    let n = inner.next_id.fetch_add(1, Ordering::Relaxed);
    let id = n.to_string();
    let (tx, rx) = oneshot::channel();
    inner.pending.lock().unwrap().insert(id.clone(), tx);
    let _pending = PendingGuard { inner, id };
    let msg = serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": n,
        "method": method,
        "params": params
    }))?;
    if inner.writer.send(msg).await.is_err() {
        return Err(StdioError::NotRunning {
            name: inner.name.clone(),
        });
    }
    let received = match request_timeout(method) {
        Some(limit) => match tokio::time::timeout(limit, rx).await {
            Err(_) => {
                return Err(StdioError::Timeout {
                    name: inner.name.clone(),
                    method: method.to_owned(),
                    timeout: limit,
                });
            }
            Ok(received) => received,
        },
        None => rx.await,
    };
    match received {
        Err(_) => Err(StdioError::Dropped {
            name: inner.name.clone(),
            method: method.to_owned(),
        }),
        Ok(Err(message)) => Err(StdioError::Remote {
            name: inner.name.clone(),
            method: method.to_owned(),
            message,
        }),
        Ok(Ok(value)) => Ok(value),
    }
}

/// Transport timeout for a host→plugin request. `tool/call` and `vfs/*`
/// have none: their deadline is the tool call's own, and the kernel cancels
/// it by dropping the request future.
fn request_timeout(method: &str) -> Option<Duration> {
    (method != "tool/call" && !method.starts_with("vfs/")).then_some(REQUEST_TIMEOUT)
}

fn id_key(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// plugin→host requests
// ---------------------------------------------------------------------------

async fn handle_plugin_request(inner: Arc<Inner>, id: Value, msg: Value) {
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let response = match handle_host_method(&inner, &method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => json!({
            "jsonrpc": "2.0", "id": id,
            "error": { "code": -32000, "message": format!("{err}") }
        }),
    };
    if let Ok(line) = serde_json::to_string(&response) {
        let _ = inner.writer.send(line).await;
    }
}

async fn handle_host_method(inner: &Inner, method: &str, params: &Value) -> Result<Value> {
    if method == "host/post" {
        let branch: rumu_core::BranchId = params
            .get("branch")
            .and_then(Value::as_str)
            .ok_or(StdioError::MissingBranch)?
            .into();
        let post: Post = serde_json::from_value(params.clone()).map_err(|source| {
            StdioError::InvalidPayload {
                what: "`host/post`",
                source,
            }
        })?;
        let inbox = inner
            .inbox
            .lock()
            .unwrap()
            .clone()
            .ok_or(StdioError::NoInbox)?;
        inbox.post(&branch, post)?;
        return Ok(Value::Null);
    }
    if method == "host/emit" {
        let event = params
            .get("event")
            .and_then(Value::as_str)
            .ok_or(StdioError::MissingEvent)?;
        let data = params.get("data").cloned().unwrap_or(Value::Null);
        let branch = match (
            params.get("branch").and_then(Value::as_str),
            params.get("ctx"),
        ) {
            (Some(b), _) => Some(rumu_core::BranchId::from(b)),
            (None, Some(Value::String(token))) => {
                let cx = inner.ctxs.lock().unwrap().get(token).cloned();
                let cx = cx.ok_or_else(|| StdioError::UnknownCtx {
                    token: token.clone(),
                })?;
                Some(cx.branch)
            }
            _ => None,
        };
        let inbox = inner
            .inbox
            .lock()
            .unwrap()
            .clone()
            .ok_or(StdioError::NoInbox)?;
        inbox.emit(event, data, branch.as_ref());
        return Ok(Value::Null);
    }
    let token = params
        .get("ctx")
        .and_then(Value::as_str)
        .ok_or(StdioError::MissingCtx)?;
    let cx = inner
        .ctxs
        .lock()
        .unwrap()
        .get(token)
        .cloned()
        .ok_or_else(|| StdioError::UnknownCtx {
            token: token.to_owned(),
        })?;
    match method {
        "host/path" => Ok(serde_json::to_value(&*cx.path()?)?),
        "host/state" => Ok(serde_json::to_value(&*cx.state()?)?),
        "host/progress" => {
            let append = params.get("append").and_then(Value::as_str);
            let data = params
                .get("data")
                .filter(|v| !v.is_null())
                .cloned()
                .map(serde_json::from_value::<rumu_core::Tagged>)
                .transpose()
                .map_err(|source| StdioError::InvalidProgress { source })?;
            match (append, data) {
                (Some(text), _) => cx.progress(Progress::Append(text.to_owned())),
                (None, Some(tagged)) => cx.progress(Progress::Data(tagged)),
                (None, None) => return Err(StdioError::ProgressNeedsArg.into()),
            }
            Ok(Value::Null)
        }
        "host/index" => {
            let tag = params
                .get("tag")
                .and_then(Value::as_str)
                .ok_or(StdioError::MissingTag)?;
            let index = cx.index()?;
            Ok(serde_json::to_value(
                index.by_tag(tag).cloned().collect::<Vec<_>>(),
            )?)
        }
        other => Err(StdioError::UnknownHostMethod {
            method: other.to_owned(),
        }
        .into()),
    }
}

// ---------------------------------------------------------------------------
// Plugin impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Plugin for StdioPlugin {
    fn manifest(&self) -> Manifest {
        self.inner.manifest.read().unwrap().clone()
    }

    fn attach(&self, inbox: Arc<dyn Inbox>) {
        *self.inner.inbox.lock().unwrap() = Some(inbox);
    }

    async fn on_hook(
        &self,
        hook_id: &str,
        event: &Event,
        cx: &Cx,
    ) -> rumu_core::Result<HookOutput> {
        let ctx = self.begin_ctx(cx);
        let params = json!({
            "hook_id": hook_id,
            "event": event,
            "ctx": ctx.token
        });
        let result = request(&self.inner, "hook", params).await;
        drop(ctx);
        // Transport failures and malformed output are errors, not a silent
        // `Pass`: the kernel fails closed on interceptable events.
        let value = result?;
        serde_json::from_value(value).map_err(|e| {
            rumu_core::Error::msg(format!("invalid HookOutput from hook `{hook_id}`: {e}"))
        })
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        let ctx = self.begin_ctx(cx);
        let params = json!({
            "name": name,
            "args": args,
            "ctx": ctx.token,
            "session": cx.session,
            "branch": cx.branch,
            "background": cx.call().is_some_and(|c| c.background)
        });
        let result = request(&self.inner, "tool/call", params).await;
        drop(ctx);
        let out = match result {
            Ok(value) => serde_json::from_value(value).unwrap_or_else(|err| {
                ToolOutput::err(format!(
                    "invalid ToolOutput from plugin `{}`: {err}",
                    self.inner.name
                ))
            }),
            Err(err) => ToolOutput::err(format!("plugin `{}`: {err}", self.inner.name)),
        };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeRuntime, fixture_home};
    use rumu_core::{BranchId, Event, Fields, SessionId};

    fn event(kind: &str, fields: Fields, payload: Value) -> Event {
        Event {
            kind: kind.into(),
            session: SessionId::new(),
            branch: BranchId::new(),
            fields: Fields {
                event: kind.into(),
                ..fields
            },
            payload,
        }
    }

    fn argv(home: &Path, script: &str) -> Vec<String> {
        vec![
            "python3".into(),
            home.join("plugins")
                .join(script)
                .to_string_lossy()
                .into_owned(),
        ]
    }

    #[tokio::test]
    async fn clock_plugin_lifecycle() {
        let home = fixture_home();
        let plugin = StdioPlugin::spawn(
            "clock",
            &argv(&home, "clock.py"),
            &home,
            &Default::default(),
        )
        .await
        .unwrap();

        let manifest = plugin.manifest();
        assert_eq!(manifest.name, "clock");
        let tool_names: Vec<&str> = manifest.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(tool_names, ["now"]);

        let host = FakeRuntime::arc();
        let cx = host.cx();

        let out = plugin.call_tool("now", json!({}), &cx).await.unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        // ISO-8601-ish, e.g. 2026-10-18T14:03:55+08:00
        assert!(
            out.content.text.contains('T') && out.content.text.contains('-'),
            "{}",
            out.content.text
        );

        // Unknown tool → JSON-RPC error surfaced as an error ToolOutput.
        let out = plugin.call_tool("bogus", json!({}), &cx).await.unwrap();
        assert!(out.is_error, "{}", out.content.text);

        // Clock's `log-events`: fires, logs to stderr, no effects.
        let ev = event(
            "prompt",
            Fields {
                text: Some("hello world".into()),
                ..Default::default()
            },
            json!({ "text": "hello world" }),
        );
        let out = plugin.on_hook("log-events", &ev, &cx).await.unwrap();
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn compact_plugin_rewrites_git_status() {
        let home = fixture_home();
        let plugin = StdioPlugin::spawn(
            "compact",
            &argv(&home, "compact.py"),
            &home,
            &Default::default(),
        )
        .await
        .unwrap();
        let manifest = plugin.manifest();
        assert_eq!(manifest.hooks.len(), 1);
        assert_eq!(manifest.hooks[0].id, "rewrite-git-status");

        let host = FakeRuntime::arc();
        let cx = host.cx();
        let ev = event(
            "tool.pre",
            Fields {
                tool: Some("shell".into()),
                cmd: Some("git status".into()),
                ..Default::default()
            },
            json!({ "name": "shell", "args": { "command": "git status" } }),
        );
        let out = plugin
            .on_hook("rewrite-git-status", &ev, &cx)
            .await
            .unwrap();
        match &out.decision {
            rumu_core::Decision::Rewrite { patch } => {
                assert_eq!(
                    patch,
                    &json!({ "args": { "command": "git status --short --branch" } })
                );
            }
            other => panic!("expected Rewrite, got {other:?}"),
        }

        // A non-matching command: hook still fires (kernel matched on cmd), but
        // the plugin returns Pass.
        let ev = event(
            "tool.pre",
            Fields {
                tool: Some("shell".into()),
                cmd: Some("echo hi".into()),
                ..Default::default()
            },
            json!({ "name": "shell", "args": { "command": "echo hi" } }),
        );
        let out = plugin
            .on_hook("rewrite-git-status", &ev, &cx)
            .await
            .unwrap();
        assert!(matches!(out.decision, rumu_core::Decision::Pass));
    }

    /// A plugin that sends a `host/state` request *while* its `tool/call`
    /// response is pending — the reader task must dispatch it independently,
    /// serving it from the Cx registered for that request's `ctx` token.
    #[tokio::test]
    async fn host_requests_served_during_tool_call() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hostq.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

MANIFEST = {
    "name": "hostq",
    "tools": [{
        "name": "workspace",
        "description": "default workspace via host/state",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }],
    "hooks": [],
}

for line in sys.stdin.buffer:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif m == "tool/call":
        token = req.get("params", {}).get("ctx")
        send({"jsonrpc": "2.0", "id": "q1", "method": "host/state",
              "params": {"ctx": token}})
        while True:
            raw = sys.stdin.buffer.readline()
            if not raw:
                raise SystemExit(1)
            r = json.loads(raw)
            if r.get("id") == "q1":
                ws = r.get("result", {}).get("default_workspace") or ""
                break
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"content": {"text": "workspace=" + ws}, "is_error": False, "effects": []}})
"#,
        )
        .unwrap();

        let argv = vec!["python3".into(), script.to_string_lossy().into_owned()];
        let plugin = StdioPlugin::spawn("hostq", &argv, dir.path(), &Default::default())
            .await
            .unwrap();

        let host = FakeRuntime::arc();
        host.state.lock().unwrap().default_workspace = Some("demo".into());
        let cx = host.cx();

        let out = plugin.call_tool("workspace", json!({}), &cx).await.unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        assert_eq!(out.content.text, "workspace=demo");
    }

    /// `host/progress` feeds the in-flight call's `cx.progress`.
    #[tokio::test]
    async fn host_progress_reaches_the_cx() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("prog.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

MANIFEST = {
    "name": "prog",
    "tools": [{
        "name": "stream",
        "description": "report progress",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }],
    "hooks": [],
}

for line in sys.stdin.buffer:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif m == "tool/call":
        token = req.get("params", {}).get("ctx")
        for i, note in enumerate([
            {"ctx": token, "append": "one"},
            {"ctx": token, "data": {"tag": "x", "v": 1, "data": {"n": 2}}},
        ]):
            send({"jsonrpc": "2.0", "id": f"p{i}", "method": "host/progress", "params": note})
            while True:
                r = json.loads(sys.stdin.buffer.readline())
                if r.get("id") == f"p{i}":
                    assert r.get("result") is None
                    break
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"content": {"text": "streamed"}, "is_error": False, "effects": []}})
"#,
        )
        .unwrap();

        let argv = vec!["python3".into(), script.to_string_lossy().into_owned()];
        let plugin = StdioPlugin::spawn("prog", &argv, dir.path(), &Default::default())
            .await
            .unwrap();
        let host = FakeRuntime::arc();
        let out = plugin
            .call_tool("stream", json!({}), &host.cx())
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        let updates = host.progress.lock().unwrap();
        assert_eq!(updates.len(), 2);
        assert!(matches!(&updates[0].2, rumu_core::Progress::Append(s) if s == "one"));
        assert!(matches!(&updates[1].2, rumu_core::Progress::Data(t) if t.tag == "x"));
    }

    /// A plugin's `ToolOutput` may carry image attachments; the host must
    /// round-trip them (the output cap touches text only).
    #[tokio::test]
    async fn tool_output_attachments_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("pic.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

MANIFEST = {
    "name": "pic",
    "tools": [{
        "name": "snap",
        "description": "returns an image attachment",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }],
    "hooks": [],
}

for line in sys.stdin.buffer:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif m == "tool/call":
        send({"jsonrpc": "2.0", "id": rid, "result": {
            "content": {"text": "see", "attachments": [{
                "hash": "ab" * 32, "mime": "image/png", "name": "a.png",
                "width": 4, "height": 3, "size": 10}]},
            "is_error": False, "effects": []}})
"#,
        )
        .unwrap();

        let argv = vec!["python3".into(), script.to_string_lossy().into_owned()];
        let plugin = StdioPlugin::spawn("pic", &argv, dir.path(), &Default::default())
            .await
            .unwrap();
        let host = FakeRuntime::arc();
        let out = plugin
            .call_tool("snap", json!({}), &host.cx())
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        assert_eq!(out.content.text, "see");
        assert_eq!(out.content.attachments.len(), 1);
        let a = &out.content.attachments[0];
        assert_eq!(a.hash, "ab".repeat(32));
        assert_eq!(a.mime, "image/png");
        assert_eq!(a.name.as_deref(), Some("a.png"));
        assert_eq!((a.width, a.height, a.size), (4, 3, 10));
    }

    /// Cancelling a tool call (dropping its future) removes its pending entry
    /// instead of leaking it.
    #[tokio::test]
    async fn dropped_tool_call_removes_its_pending_entry() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("stall.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

MANIFEST = {
    "name": "stall",
    "tools": [{
        "name": "wait",
        "description": "never answers",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }],
    "hooks": [],
}

for line in sys.stdin.buffer:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif m == "tool/call":
        pass  # never respond
"#,
        )
        .unwrap();

        let argv = vec!["python3".into(), script.to_string_lossy().into_owned()];
        let plugin = StdioPlugin::spawn("stall", &argv, dir.path(), &Default::default())
            .await
            .unwrap();
        let host = FakeRuntime::arc();
        let cx = host.cx();

        let call = plugin.call_tool("wait", json!({}), &cx);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), call)
                .await
                .is_err(),
            "the call must still be waiting (no transport timeout)"
        );
        assert!(plugin.inner.pending.lock().unwrap().is_empty());
    }

    /// `host/post` targets an explicit branch and needs no `ctx` token.
    #[tokio::test]
    async fn host_post_needs_no_ctx_token() {
        use crate::testutil::FakeInbox;

        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("poster.py");
        std::fs::write(
            &script,
            r#"
import json, sys

def send(m):
    sys.stdout.buffer.write((json.dumps(m) + "\n").encode("utf-8"))
    sys.stdout.buffer.flush()

MANIFEST = {
    "name": "poster",
    "tools": [{
        "name": "postit",
        "description": "post out of band",
        "schema": {"type": "object", "properties": {}, "additionalProperties": False},
    }],
    "hooks": [],
}

for line in sys.stdin.buffer:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    m = req.get("method"); rid = req.get("id")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": MANIFEST})
    elif m == "tool/call":
        send({"jsonrpc": "2.0", "id": "q1", "method": "host/post",
              "params": {"branch": "B1", "effects": [{"text": "hello"}], "wake": True}})
        while True:
            raw = sys.stdin.buffer.readline()
            if not raw:
                raise SystemExit(1)
            r = json.loads(raw)
            if r.get("id") == "q1":
                assert r.get("result") is None
                break
        send({"jsonrpc": "2.0", "id": rid,
              "result": {"content": {"text": "posted"}, "is_error": False, "effects": []}})
"#,
        )
        .unwrap();

        let argv = vec!["python3".into(), script.to_string_lossy().into_owned()];
        let plugin = StdioPlugin::spawn("poster", &argv, dir.path(), &Default::default())
            .await
            .unwrap();
        let inbox = FakeInbox::arc();
        plugin.attach(inbox.clone());

        let host = FakeRuntime::arc();
        let out = plugin
            .call_tool("postit", json!({}), &host.cx())
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        let posts = inbox.posts();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].0, BranchId::from("B1"));
        assert!(posts[0].1.wake);
        assert_eq!(posts[0].1.effects[0].text, "hello");
    }
}
