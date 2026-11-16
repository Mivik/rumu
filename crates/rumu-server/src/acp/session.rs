//! One ACP session: the main line of a rumu session, what its prompts wait
//! for, and how a client's working directory becomes its workspace.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rumu_core::{PathState, Usage, Workspace};
use serde_json::{Value, json};
use tokio::sync::watch;

use super::Rumu;
use super::config::Config;
use super::events::{Event, Translator};

/// How long a prompt waits, once the branch is idle, for its last pushes to
/// be translated (they always are, unless the push stream lagged).
const FLUSH_WAIT: Duration = Duration::from_secs(5);

/// How a prompt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stop {
    /// The branch went idle.
    EndTurn,
    /// A hook refused the prompt.
    Refusal,
    /// `session/cancel` stopped it, or it was cancelled while queued.
    Cancelled,
}

/// A prompt's outcome, with the tokens spent while it ran.
pub(super) struct Outcome {
    pub stop: Stop,
    pub usage: Usage,
}

/// The rumu session behind an ACP session. The assistant can still change
/// before the first prompt, which replaces it.
struct Bind {
    session: String,
    branch: String,
    translator: Translator,
}

/// A turn that failed on the main line.
struct Failure {
    message: String,
    blocked: bool,
}

pub(super) struct Session {
    /// The ACP session id: the id of the first rumu session behind it.
    pub id: String,
    /// The working directory's workspace and the project it is in.
    place: Place,
    bind: Mutex<Bind>,
    pub config: Mutex<Config>,
    /// Set by the first prompt.
    started: AtomicBool,
    /// One prompt at a time; the others queue here in order.
    slot: tokio::sync::Mutex<()>,
    /// Bumped by every `session/cancel`: prompts started or queued before
    /// one end as cancelled.
    cancels: AtomicU64,
    /// Bumped as each `branch.idle` of the main line is translated.
    idle: watch::Sender<u64>,
    failure: Mutex<Option<Failure>>,
    /// Tokens spent by every branch of the session.
    usage: Mutex<Usage>,
}

/// Where a session works: the client's directory as a workspace, in a
/// project.
#[derive(Clone)]
struct Place {
    workspace: String,
    root: String,
    project: String,
    /// Whether the workspace is the project's default.
    default: bool,
}

impl Session {
    /// A new session working in `cwd`, as `assistant` (the default one when
    /// `None`).
    pub async fn create(rumu: &Rumu, cwd: &Path, config: Config) -> Result<Arc<Self>, String> {
        let place = place(rumu, cwd).await?;
        let (session, branch) = open(rumu, &place, config.assistant.as_deref()).await?;
        Ok(Arc::new(Self {
            id: session.clone(),
            place,
            bind: Mutex::new(Bind {
                session,
                translator: Translator::new(branch.clone()),
                branch,
            }),
            config: Mutex::new(config),
            started: AtomicBool::new(false),
            slot: tokio::sync::Mutex::new(()),
            cancels: AtomicU64::new(0),
            idle: watch::channel(0).0,
            failure: Mutex::new(None),
            usage: Mutex::new(Usage::default()),
        }))
    }

    /// The main line's branch id.
    pub fn branch(&self) -> String {
        self.bind.lock().unwrap().branch.clone()
    }

    /// The rumu session's id.
    pub fn rumu_session(&self) -> String {
        self.bind.lock().unwrap().session.clone()
    }

    /// Whether a prompt was ever sent.
    pub fn started(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Start over on a new rumu session as `assistant`: only before the
    /// first prompt (the old session is deleted).
    pub async fn rebind(&self, rumu: &Rumu, assistant: Option<&str>) -> Result<(), String> {
        if self.started() {
            return Err("the assistant is fixed once the session has a prompt".into());
        }
        let (session, branch) = open(rumu, &self.place, assistant).await?;
        let old = {
            let mut bind = self.bind.lock().unwrap();
            let old = std::mem::replace(&mut bind.session, session);
            bind.translator = Translator::new(branch.clone());
            bind.branch = branch;
            old
        };
        rumu.call("session.delete", json!({ "session": old }))
            .await
            .map(|_| ())
    }

    /// Whether the push concerns this session's main line, and carries
    /// tool calls whose paths need its state to resolve.
    pub fn needs_state(&self, method: &str, params: &Value) -> bool {
        method == "node.appended"
            && self.bind.lock().unwrap().translator.on_main(params)
            && params
                .pointer("/node/body/tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty())
    }

    /// Take in one push: keep the session's books, and return what the
    /// client should see of it.
    pub fn feed(&self, method: &str, params: &Value, state: Option<&PathState>) -> Vec<Event> {
        let mut bind = self.bind.lock().unwrap();
        let main = bind.translator.on_main(params);
        match method {
            "turn.started" if main => *self.failure.lock().unwrap() = None,
            "turn.error" if main => {
                *self.failure.lock().unwrap() = Some(Failure {
                    message: params
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("the turn failed")
                        .to_owned(),
                    blocked: params.get("blocked").and_then(Value::as_bool) == Some(true),
                });
            }
            "branch.idle" if main => self.idle.send_modify(|n| *n += 1),
            "node.appended"
                if params.pointer("/node/session").and_then(Value::as_str)
                    == Some(bind.session.as_str()) =>
            {
                if let Some(Ok(u)) = params
                    .pointer("/node/body/usage")
                    .cloned()
                    .map(serde_json::from_value::<Usage>)
                {
                    let mut usage = self.usage.lock().unwrap();
                    usage.prompt_tokens += u.prompt_tokens;
                    usage.completion_tokens += u.completion_tokens;
                    usage.cached_tokens += u.cached_tokens;
                    usage.cache_creation_tokens += u.cache_creation_tokens;
                }
            }
            _ => {}
        }
        bind.translator.feed(method, params, state)
    }

    /// Run one prompt: queued behind the ones before it, then sent, then
    /// waited on until the main line is idle — no turn running or about to
    /// start, no background sub-agent left (see `Kernel::is_idle`) — and
    /// every push of that work has been translated.
    pub async fn prompt(&self, rumu: &Rumu, text: String) -> Result<Outcome, String> {
        let generation = self.cancels.load(Ordering::SeqCst);
        let _slot = self.slot.lock().await;
        let cancelled = || self.cancels.load(Ordering::SeqCst) != generation;
        if cancelled() {
            return Ok(Outcome {
                stop: Stop::Cancelled,
                usage: Usage::default(),
            });
        }
        self.started.store(true, Ordering::SeqCst);
        *self.failure.lock().unwrap() = None;
        let before = self.usage.lock().unwrap().clone();
        let branch = self.branch();
        let mut idle = self.idle.subscribe();
        let model = self.config.lock().unwrap().choice();
        rumu.call(
            "prompt",
            json!({ "branch": branch, "text": text, "model": model }),
        )
        .await?;
        rumu.call("branch.wait_idle", json!({ "branch": branch }))
            .await?;
        if tokio::time::timeout(FLUSH_WAIT, idle.changed())
            .await
            .is_err()
        {
            tracing::warn!(%branch, "the idle push never arrived; answering anyway");
        }
        let usage = {
            let now = self.usage.lock().unwrap();
            Usage {
                prompt_tokens: now.prompt_tokens - before.prompt_tokens,
                completion_tokens: now.completion_tokens - before.completion_tokens,
                cached_tokens: now.cached_tokens - before.cached_tokens,
                cache_creation_tokens: now.cache_creation_tokens - before.cache_creation_tokens,
            }
        };
        if cancelled() {
            return Ok(Outcome {
                stop: Stop::Cancelled,
                usage,
            });
        }
        match self.failure.lock().unwrap().take() {
            Some(f) if f.blocked => Ok(Outcome {
                stop: Stop::Refusal,
                usage,
            }),
            Some(f) => Err(f.message),
            None => Ok(Outcome {
                stop: Stop::EndTurn,
                usage,
            }),
        }
    }

    /// `session/cancel`: stop the main line (its background work too), and
    /// every prompt sent or queued so far.
    pub async fn cancel(&self, rumu: &Rumu) -> Result<(), String> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        rumu.call("abort", json!({ "branch": self.branch() }))
            .await
            .map(|_| ())
    }

    /// Whether a prompt is in flight.
    pub fn busy(&self) -> bool {
        self.slot.try_lock().is_err()
    }

    /// Send `text` now: into the running turn when there is one, else as a
    /// turn of its own that no prompt waits for.
    pub async fn steer(&self, rumu: &Rumu, text: String) -> Result<(), String> {
        let model = self.config.lock().unwrap().choice();
        self.started.store(true, Ordering::SeqCst);
        rumu.call(
            "prompt",
            json!({ "branch": self.branch(), "text": text, "model": model }),
        )
        .await
        .map(|_| ())
    }
}

/// The client's working directory as a registered workspace (registered now
/// when it is not), and the project it goes in: the first that mounts it, or
/// a new one of its name.
async fn place(rumu: &Rumu, cwd: &Path) -> Result<Place, String> {
    if !cwd.is_absolute() {
        return Err(format!("`cwd` must be an absolute path: {}", cwd.display()));
    }
    // Trailing slashes and `.` segments dropped.
    let root: PathBuf = cwd
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect();
    let root = root.to_string_lossy().into_owned();
    let list: Vec<Workspace> =
        serde_json::from_value(rumu.call("workspace.list", json!({})).await?)
            .map_err(|e| e.to_string())?;
    let workspace = match list
        .iter()
        .find(|w| w.is_local() && w.root.as_str() == root)
    {
        Some(w) => w.name.clone(),
        None => {
            let base = rumu_plugins::cwd_workspace_name(&root);
            let name = (1..)
                .map(|i| {
                    if i == 1 {
                        base.clone()
                    } else {
                        format!("{base}-{i}")
                    }
                })
                .find(|n| !list.iter().any(|w| &w.name == n))
                .unwrap_or(base);
            rumu.call("workspace.upsert", json!({ "name": name, "root": root }))
                .await?;
            name
        }
    };
    let projects = rumu.call("project.list", json!({})).await?;
    let mounting = projects.as_array().into_iter().flatten().find(|p| {
        p.get("workspaces")
            .and_then(Value::as_array)
            .is_some_and(|ws| ws.iter().any(|w| w.as_str() == Some(workspace.as_str())))
    });
    let (project, default) = match mounting {
        Some(p) => (
            p.get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            p.pointer("/workspaces/0").and_then(Value::as_str) == Some(workspace.as_str()),
        ),
        None => {
            let mut last = String::new();
            let mut created = None;
            for i in 1..=10 {
                let name = if i == 1 {
                    workspace.clone()
                } else {
                    format!("{workspace}-{i}")
                };
                match rumu
                    .call(
                        "project.create",
                        json!({ "name": name, "workspaces": [workspace] }),
                    )
                    .await
                {
                    Ok(v) => {
                        created = v
                            .pointer("/project/id")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        break;
                    }
                    Err(e) => last = e,
                }
            }
            (created.ok_or(last)?, true)
        }
    };
    Ok(Place {
        workspace,
        root,
        project,
        default,
    })
}

/// A rumu session in `place`, as `assistant`: its id and main branch.
async fn open(
    rumu: &Rumu,
    place: &Place,
    assistant: Option<&str>,
) -> Result<(String, String), String> {
    let mut params = json!({ "project": place.project });
    if let Some(assistant) = assistant {
        params["assistant"] = json!(assistant);
    }
    let created = rumu.call("session.create", params).await?;
    let id = |pointer: &str| {
        created
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("session.create returned no {pointer}"))
    };
    let (session, branch) = (id("/session/id")?, id("/branch/id")?);
    if !place.default {
        let text = format!(
            "The client works in `{}` ({}); it is now the default workspace.",
            place.workspace, place.root
        );
        rumu.call(
            "branch.use_workspace",
            json!({ "branch": branch, "name": place.workspace, "text": text }),
        )
        .await?;
    }
    Ok((session, branch))
}
