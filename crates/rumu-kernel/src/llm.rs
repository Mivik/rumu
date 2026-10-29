//! The kernel's LLM boundary.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{Node, ReasoningBlock, ToolCall, ToolSpec, Usage};

/// A model-call failure.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// `RUMU_MODEL` was not configured.
    #[error("RUMU_MODEL is not set; provide the model id your endpoint expects")]
    MissingModel,

    /// `RUMU_EFFORT` is not a reasoning effort the adapter knows.
    #[error("RUMU_EFFORT must be one of low, medium, high, xhigh, max (got {0:?})")]
    InvalidEffort(String),

    /// The request could not be sent.
    #[error("llm request failed: {0}")]
    Request(String),

    /// The response stream failed part-way.
    #[error("llm stream failed: {0}")]
    Stream(String),

    /// The stream ended without a final event.
    #[error("llm stream ended without an End event")]
    MissingEnd,

    /// The adapter client could not be built.
    #[error("failed to build the LLM client: {0}")]
    Client(String),
}

/// A streaming delta from the model.
#[derive(Debug, Clone)]
pub enum Delta {
    Text(String),
    Reasoning(String),
}

pub type OnDelta = Arc<dyn Fn(Delta) + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    pub text: String,
    pub reasoning: Vec<ReasoningBlock>,
    /// Wall time spent reasoning (first reasoning delta → first text delta,
    /// or stream end when there is no text), ms.
    pub reason_ms: Option<u64>,
    /// Earlier reasoning was left out of this request because the provider
    /// rejected or dropped it; recorded on the Assistant node.
    pub reasoning_reset: bool,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    /// Model identifier stamped on the Assistant node.
    pub model: String,
}

impl LlmResponse {
    /// A plain text reply.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// A reply that asks for tool calls (empty text).
    pub fn calls(calls: impl IntoIterator<Item = ToolCall>) -> Self {
        Self {
            tool_calls: calls.into_iter().collect(),
            ..Self::default()
        }
    }
}

/// One completion over the projection of a branch path.
#[async_trait]
pub trait Llm: Send + Sync {
    /// `system` is the frozen system prompt, `tools` the frozen sorted tool
    /// list, `nodes` the root→head path. `on_delta` (if set) receives
    /// streaming deltas. Nothing is persisted by the implementation.
    async fn complete(
        &self,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError>;
}
