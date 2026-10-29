use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BranchId, SessionId};

/// Event kinds emitted by the kernel.
pub mod kind {
    pub const SESSION_START: &str = "session.start";
    /// payload: `{ "text": .. }` — interceptable (block / rewrite)
    pub const PROMPT: &str = "prompt";
    /// Before each LLM request; hooks typically `inject`.
    pub const LLM_PRE: &str = "llm.pre";
    /// payload: `{ "text": .., "tool_calls": [..], "usage": {..} }`
    pub const LLM_DONE: &str = "llm.done";
    /// payload: `{ "name": .., "args": {..} }` — interceptable
    pub const TOOL_PRE: &str = "tool.pre";
    /// payload: `{ "name": .., "args": {..}, "output": .., "is_error": .. }`
    pub const TOOL_POST: &str = "tool.post";
    /// An out-of-band post to a branch, before delivery.
    /// payload: `{ "source": .., "effects": [..], "wake": bool }` — interceptable
    pub const POST: &str = "post";
    pub const BRANCH_FORK: &str = "branch.fork";
    pub const BRANCH_REVERT: &str = "branch.revert";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub kind: String,
    pub session: SessionId,
    pub branch: BranchId,
    /// Normalized fields that hook `match` predicates run against.
    pub fields: Fields,
    /// Raw event payload; `rewrite` actions JSON-merge-patch this.
    pub payload: Value,
}

/// Normalized view of an event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Fields {
    /// Same as `Event::kind`.
    pub event: String,
    pub tool: Option<String>,
    /// `args.command` of a tool call, or `args.code` for the `code` tool.
    pub cmd: Option<String>,
    /// Who issued the tool call: `"model"` or the invoking plugin's manifest
    /// name (e.g. `"code"` for a script's internal call). Set on `tool.pre` /
    /// `tool.post` only; also the poster's name on `post`.
    pub caller: Option<String>,
    /// The source plugin of a `post` event.
    pub source: Option<String>,
    /// The target of a tool call (`tool.pre`/`tool.post` only), as a single
    /// name: a workspace preset name or `@rumu`. None when the target is
    /// unknown/unresolvable and on every other event.
    pub workspace: Option<String>,
    /// The normalized absolute paths of a tool call's `path`(+`paths`) args
    /// (file tools only): on the workspace's machine in its native style
    /// (`/home/u/code/proj/src/a.rs`, `C:\Users\me\proj\src\a.rs`), or
    /// `/cards/<id>` / `/sessions/…` on `@rumu`. The target itself is
    /// `workspace`, not a path — `bash`, which has no `path`, is matchable
    /// via `workspace`.
    pub paths: Vec<String>,
    /// User prompt text / assistant text.
    pub text: Option<String>,
    /// Tool output.
    pub output: Option<String>,
}
