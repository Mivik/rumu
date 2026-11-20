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

    /// The task belongs to neither the branch nor one of its sub-agents.
    #[error("task `{id}` is not owned by this branch or its sub-agents")]
    TaskNotOwned { id: TaskId },

    /// The parent branch has no assistant node containing the agent call.
    #[error("agent: no assistant node containing call `{call_id}` on branch {branch}")]
    AgentCallNotFound { call_id: String, branch: BranchId },

    // ---- guards / request validation ----
    /// A turn is already running on the branch.
    #[error("busy: a turn is already running on this branch")]
    TurnBusy,

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

    /// `prompt.cancel` named a prompt no longer queued: it has been written
    /// for the model to see.
    #[error("the prompt is no longer queued")]
    NotQueued,

    /// `branch.move` or `side` was given a branch that is not a main line (a
    /// sub-agent's or a side question's).
    #[error("branch {0} is not a main line")]
    NotMainLine(BranchId),

    /// `branch.move` while background tasks or sub-agents run that came from
    /// a call the new line would not hold.
    #[error(
        "busy: these came from a call the line would leave and are still running: {0}; stop \
         them first"
    )]
    LineBusy(String),

    /// `branch.move` was given a node a sub-agent or side question added.
    #[error("node {0} is not on the main line")]
    OffTheLine(NodeId),

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

    /// A title was asked for while no model writes them.
    #[error("no model writes session titles; pick one in Settings")]
    NoTitleModel,

    /// A sub-agent's `model` / `effort` that it cannot run on.
    #[error("{0}")]
    ModelPick(String),

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
