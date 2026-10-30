use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BranchId, NodeId, SessionId, Timestamp, Workspace};

/// A persisted event. All nodes of a session form a tree via `parent`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub session: SessionId,
    /// End time of the producing operation (append-on-complete).
    pub at: Timestamp,
    /// Wall time of the operation that produced this node, ms. `at` is the end
    /// time, so the start is `at - dur_ms`; instantaneous nodes (user, note,
    /// marker) leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_ms: Option<u64>,
    pub body: NodeBody,
}

/// Versioned structured payload. Readers that don't know `tag` or don't support
/// `v` fall back to the node's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Tagged {
    pub tag: String,
    pub v: u32,
    pub data: Value,
}

/// A payload type with a stable tag and version, stored in [`Tagged`].
pub trait Structured: Serialize + DeserializeOwned {
    const TAG: &'static str;
    const V: u32;
}

impl Tagged {
    /// Encode a structured payload.
    pub fn new<T: Structured>(data: &T) -> Self {
        Self {
            tag: T::TAG.to_owned(),
            v: T::V,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
        }
    }

    /// Decode the payload, or `None` on a tag/version mismatch or a parse error.
    pub fn decode<T: Structured>(&self) -> Option<T> {
        if self.tag != T::TAG || self.v != T::V {
            return None;
        }
        serde_json::from_value(self.data.clone()).ok()
    }
}

/// The note `source` for notes written by rumu itself (the kernel), not by a
/// plugin.
pub const SYSTEM_SOURCE: &str = "system";

/// Everything projected to the model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Content {
    /// Frozen at write time; the ONLY thing the model sees.
    pub text: String,
    /// For the UI and plugins; never projected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Tagged>,
}

impl Content {
    /// Plain text content.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            data: None,
        }
    }

    /// Note text plus a structured payload.
    pub fn note_with<T: Structured>(s: impl Into<String>, data: &T) -> Self {
        Self::text(s).with_data(data)
    }

    /// A marker: no text (never projected) plus a structured payload, visible
    /// only through the node index and structured readers.
    pub fn marker<T: Structured>(data: &T) -> Self {
        Self {
            text: String::new(),
            data: Some(Tagged::new(data)),
        }
    }

    /// Plain text plus a structured payload.
    pub fn with_data<T: Structured>(mut self, data: &T) -> Self {
        self.data = Some(Tagged::new(data));
        self
    }

    /// Decode the structured payload, if it has the expected tag and version.
    pub fn decode<T: Structured>(&self) -> Option<T> {
        self.data.as_ref()?.decode()
    }

    /// True when this is a marker: empty text (not projected) with a payload.
    pub fn is_marker(&self) -> bool {
        self.text.is_empty() && self.data.is_some()
    }

    /// Validate that a note carries either projected text or a structured
    /// payload. Empty-text markers are meaningful only with `data`; a note
    /// with neither is rejected at the [`crate::Inbox`] boundary.
    pub fn validate_note(&self) -> crate::Result<()> {
        if self.text.is_empty() && self.data.is_none() {
            return Err(crate::Error::EmptyNote);
        }
        Ok(())
    }
}

/// One thinking block of a model reply, in the order the reply gave them.
/// Anthropic accepts a replayed block only with the exact text and opaque
/// signature it produced, ahead of the text and tool calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ReasoningBlock {
    /// Visible (or summarized) thinking text. `signature` is the opaque
    /// continuation token; without one the block is display-only.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A provider-native opaque block, replayed verbatim (Anthropic
    /// `redacted_thinking`).
    Redacted { data: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum NodeBody {
    User {
        content: Content,
    },
    Assistant {
        content: Content,
        /// Thinking blocks, in the reply's original order. Empty on replies
        /// without reasoning.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasoning: Vec<ReasoningBlock>,
        /// Wall time the model spent reasoning before its first text token, ms.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason_ms: Option<u64>,
        /// The provider could not use earlier reasoning on this request (it
        /// no longer verified against the conversation), so the reasoning of
        /// every assistant node before this one is never replayed again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reasoning_reset: bool,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        model: String,
        #[serde(default)]
        usage: Usage,
    },
    ToolResult {
        call_id: String,
        content: Content,
        #[serde(default)]
        is_error: bool,
    },
    /// Plugin-written context (memory cards, workspace info, reminders,
    /// revert lessons, the card catalog). Non-empty `content.text` projects as
    /// a user-side `<note source="..">` fragment; empty text means a marker —
    /// not projected to the model, visible only through `content.data`.
    Note {
        source: String,
        content: Content,
    },
}

/// Payload of the `workspace.use` marker (v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorkspaceUse {
    pub name: String,
}

impl Structured for WorkspaceUse {
    const TAG: &'static str = "workspace.use";
    const V: u32 = 1;
}

/// Payload of the `workspace.scope` marker (v1). Every scope marker on a path
/// narrows the available workspaces to their intersection (nested sub-agents
/// can only narrow further); the built-in `@rumu` workspace is always allowed.
///
/// This is an addressing restriction, not a sandbox: `bash` in a local
/// workspace can still `cd` anywhere on the machine (see the workspace
/// plugin's caveat), and absolute paths are allowed inside an in-scope
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceScope {
    /// Workspace names allowed below this marker (`@rumu` implicit).
    pub allow: Vec<String>,
}

impl Structured for WorkspaceScope {
    const TAG: &'static str = "workspace.scope";
    const V: u32 = 1;
}

/// Payload of the session `root` marker (v1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRoot {}

impl Structured for SessionRoot {
    const TAG: &'static str = "root";
    const V: u32 = 1;
}

/// Payload of a revert lesson note (v1); the note text is the lesson itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertLesson {
    pub lesson: String,
}

impl Structured for RevertLesson {
    const TAG: &'static str = "kernel.revert";
    const V: u32 = 1;
}

/// Payload of the `turn.error` marker (v1): a turn that failed terminally
/// after its retries ran out. Recorded at the branch head as a durable record
/// of the failure; the marker's empty text keeps it out of the model's
/// projection, and the UI never renders it (failures are shown live only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TurnError {
    pub message: String,
}

impl Structured for TurnError {
    const TAG: &'static str = "turn.error";
    const V: u32 = 1;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

impl ToolCall {
    /// One model tool call.
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            args,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Prompt tokens read from cache.
    pub cached_tokens: u64,
    /// Prompt tokens written to cache.
    pub cache_creation_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Branch {
    pub id: BranchId,
    pub session: SessionId,
    pub head: NodeId,
    /// Node this branch was created from (its first own node has this as parent).
    pub forked_from: Option<NodeId>,
    pub kind: BranchKind,
    pub status: BranchStatus,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum BranchKind {
    Main,
    Fork,
    Agent {
        task: String,
        parent_branch: BranchId,
        /// The `agent` tool call that started this branch: the model's call
        /// id, or the internal call id (`"{parent_call}.{seq}"`) when issued
        /// from a plugin. The UI hangs the child branch off that call row.
        call_id: String,
        /// True when `call_id` is an internal call issued from a plugin
        /// (e.g. a `code` script) rather than a direct model call.
        #[serde(default)]
        via_plugin: bool,
        /// The resolved `inherit` argument the sub-agent was started with.
        #[serde(default)]
        inherit: Inherit,
    },
    Revert {
        lesson: String,
    },
}

/// How an `agent` sub-agent inherits from the branch that spawned it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Inherit {
    #[serde(default)]
    pub context: InheritContext,
    /// `None` = every workspace; `Some(names)` writes a `workspace.scope`
    /// marker so only those names resolve on the child path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<String>>,
}

/// Context inheritance for an `agent` sub-agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InheritContext {
    /// Fork at the assistant node containing the call: the child sees the
    /// whole conversation up to that point.
    #[default]
    Full,
    /// Fork at the session root: the child sees only its task.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BranchStatus {
    Active,
    Done,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SessionInfo {
    pub id: SessionId,
    pub title: String,
    pub active_branch: BranchId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

/// State derived purely from the root→head path (`state = f(path)`), merged
/// with the workspace preset catalog by the kernel (see `rumu-kernel
/// state::merge_workspaces`): `workspaces` there also contains the presets
/// and `default_workspace` is fully resolved.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PathState {
    /// Intersection of every `workspace.scope` marker on the path, when any
    /// (the kernel's merge filters `workspaces` to it). `None` = unscoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_scope: Option<Vec<String>>,
    /// Workspaces available on this path: the preset catalog, filled in by
    /// the kernel's merge (`rumu-kernel` `state::merge_workspaces`). Empty
    /// straight out of `derive_state`.
    pub workspaces: Vec<Workspace>,
    /// Default workspace name: the last `workspace.use` marker here; after
    /// the kernel's merge, resolved against the catalog (marker → preset
    /// default → first available) and kept within the workspace scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace: Option<String>,
}
