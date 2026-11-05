//! The kernel's typed errors.

use rumu_core::{BranchId, NodeId, SessionId, TaskId};

use crate::PredicateError;
use crate::llm::LlmError;

/// A kernel failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // ---- boundary passthrough ----
    /// The session store failed.
    #[error(transparent)]
    Store(#[from] rumu_store::Error),

    /// A plugin / harness call failed at the shared boundary.
    #[error(transparent)]
    Core(#[from] rumu_core::Error),

    /// The model call failed.
    #[error(transparent)]
    Llm(#[from] LlmError),

    /// A JSON value could not be built or read.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    // ---- lookups ----
    /// No session with that id is loaded.
    #[error("session {0} not found")]
    SessionNotFound(SessionId),

    /// No branch with that id is loaded.
    #[error("branch {0} not found")]
    BranchNotFound(BranchId),

    /// No node with that id exists on the branch's session.
    #[error("node {0} not found")]
    NodeNotFound(NodeId),

    /// No background task with that id exists.
    #[error("no task `{id}`")]
    TaskNotFound { id: TaskId },

    /// The parent branch has no assistant node containing the agent call.
    #[error("agent: no assistant node containing call `{call_id}` on branch {branch}")]
    AgentCallNotFound { call_id: String, branch: BranchId },

    // ---- guards / request validation ----
    /// A turn is already running on the branch.
    #[error("busy: a turn is already running on this branch")]
    TurnBusy,

    /// The session's active branch is busy.
    #[error("busy: the active branch has a running turn")]
    ActiveBranchBusy,

    /// An internal call needs a running turn on the branch.
    #[error("no running turn on branch {branch}")]
    NoRunningTurn { branch: BranchId },

    /// A branch fork point has pending tool calls.
    #[error("cannot branch at an assistant node with pending tool calls (unsafe point)")]
    UnsafeBranchPoint,

    /// `regenerate` was given a node that is not a user message.
    #[error("regenerate: node is not a user message")]
    NotAUserMessage,

    /// `retry` was given a branch with no recorded failure to replay: no
    /// `turn.error` marker behind an unanswered prompt (a hook-refused
    /// prompt records nothing), or the last exchange already has its reply.
    #[error("retry: no recorded failure to retry")]
    NothingToRetry,

    /// `activate` was given a sub-agent branch: it is run by its parent's
    /// tool call, never by the user.
    #[error("activate: branch {0} is a sub-agent branch")]
    AgentBranchNotActivatable(BranchId),

    /// The per-branch running-task limit was reached.
    #[error("branch already has {running} running tasks (limit {limit})")]
    TooManyTasks { running: usize, limit: usize },

    /// A hook matcher failed to compile.
    #[error("hook {plugin}/{hook}: {source}")]
    Hook {
        plugin: String,
        hook: String,
        #[source]
        source: PredicateError,
    },

    /// A session's default workspace is not known.
    #[error("unknown workspace `{name}` (presets: {presets}, or `@rumu`)")]
    UnknownWorkspace { name: String, presets: String },

    // ---- lifecycle / control flow ----
    /// The enclosing turn was aborted while an internal call was queued.
    #[error("internal tool call aborted")]
    InternalAborted,

    /// A question to the user ended unanswered: the turn was aborted.
    #[error("the question to the user was cancelled")]
    DecisionCancelled,

    /// `decision.answer` named no waiting decision.
    #[error("no pending decision `{0}`")]
    UnknownDecision(String),

    /// A kernel invariant was violated: a bug, or a race at shutdown.
    #[error("internal: {0}")]
    Internal(String),

    /// The kernel was built outside a Tokio runtime.
    #[error("Kernel::new must be called inside a Tokio runtime")]
    NoRuntime,

    /// The out-of-band post worker is gone.
    #[error("the kernel's post worker is not running")]
    PostWorkerGone,
}

/// A result with the kernel error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for rumu_core::Error {
    fn from(error: Error) -> Self {
        rumu_core::Error::other(error)
    }
}
