//! The kernel's LLM boundary.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{
    CallStats, CompactSettings, ModelPick, Node, ReasoningBlock, Titles, ToolCall, ToolSpec, Usage,
};

/// A model-call failure.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// No default model is configured.
    #[error("no default model is configured; set one in Settings")]
    MissingModel,

    /// A model ref whose provider is not configured.
    #[error("unknown model `{0}`")]
    UnknownModel(String),

    /// The model's provider has no endpoint for the protocol it runs on.
    #[error("no endpoint of its provider can serve `{0}`")]
    NoEndpoint(String),

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

/// Fragments a flattened genai/reqwest error uses for network-level
/// failures — no HTTP status is attached to any of them.
const NETWORK_TRANSIENTS: &[&str] = &[
    // reqwest's `Kind::Request` Display: connect refused/reset and timeouts
    // all surface as this (the cause chain is not part of the string).
    "error sending request",
    // reqwest body/stream IO errors and hyper causes, when they do appear.
    "connection reset",
    "connection refused",
    "connection closed",
    "connection aborted",
    "broken pipe",
    "body error",
    // reqwest timeouts and genai's stream header/read deadlines.
    "timed out",
    "timeout",
];

/// How providers word a request that does not fit the window.
const OVERFLOW_PHRASES: &[&str] = &[
    "prompt is too long",
    "context_length_exceeded",
    "context length",
    "context window",
    "maximum context",
    "input is too long",
    "too many input tokens",
    "input token count",
    "exceeds the maximum number of tokens",
    "request too large",
];

impl LlmError {
    /// The HTTP status embedded in a flattened `Request`/`Stream` message, if
    /// any.
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Request(m) | Self::Stream(m) => embedded_status(m),
            _ => None,
        }
    }

    /// Whether the provider refused the request as longer than the model's
    /// window: a 413, or one of the phrases providers word it with.
    pub fn is_overflow(&self) -> bool {
        let (Self::Request(m) | Self::Stream(m)) = self else {
            return false;
        };
        match embedded_status(m) {
            Some(413) => true,
            Some(429) => false,
            _ => {
                let lower = m.to_ascii_lowercase();
                OVERFLOW_PHRASES.iter().any(|f| lower.contains(f))
            }
        }
    }

    /// Whether a failed attempt might succeed if replayed: rate limiting, a
    /// server-side error, or a network-level fault. Request validation
    /// (other 4xx), missing configuration and client-build failures are the
    /// caller's fault — replaying those only burns time. `MissingEnd` counts:
    /// the provider cut the stream without concluding it.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::MissingModel | Self::UnknownModel(_) | Self::NoEndpoint(_) | Self::Client(_) => {
                false
            }
            Self::MissingEnd => true,
            Self::Request(m) | Self::Stream(m) => transient_message(m),
        }
    }
}

/// The HTTP status embedded in a flattened genai/reqwest error message, if
/// any. genai renders its failures through `Display`, so the code arrives as
/// one of three text shapes: `Status: 503 …` (its `HttpError`),
/// `status code '503'` (its `ResponseFailedStatus`) or
/// `HTTP status server error (503)` (reqwest's `error_for_status`). The first
/// mention wins: in every shape the code precedes the response body, which is
/// free text and must not be mistaken for a status.
fn embedded_status(message: &str) -> Option<u16> {
    const LEADS: [&str; 3] = ["status: ", "status code '", "error ("];
    let lower = message.to_ascii_lowercase();
    LEADS
        .iter()
        .filter_map(|lead| lower.find(lead).map(|at| (at, lead.len())))
        .min_by_key(|(at, _)| *at)
        .and_then(|(at, len)| {
            message
                .get(at + len..at + len + 3)
                .and_then(|digits| digits.parse().ok())
        })
}

/// Classify a `Request`/`Stream` message: by its HTTP status when there is
/// one (429 and any 5xx are worth replaying), otherwise by the
/// network-failure fragments.
fn transient_message(message: &str) -> bool {
    if let Some(code) = embedded_status(message) {
        return code == 429 || (500..=599).contains(&code);
    }
    let lower = message.to_ascii_lowercase();
    NETWORK_TRANSIENTS.iter().any(|f| lower.contains(f))
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
    /// First chunk and reasoning time (first reasoning delta → first text
    /// delta, or stream end when there is no text); the kernel adds the
    /// retries.
    pub stats: CallStats,
    /// Earlier reasoning was left out of this request because the provider
    /// rejected or dropped it, or because it was made by another model;
    /// recorded on the Assistant node.
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
    /// `pick` is the model to run on, `system` the frozen system prompt,
    /// `tools` the frozen sorted tool list, `nodes` the root→head path.
    /// `on_delta` (if set) receives streaming deltas. Nothing is persisted
    /// by the implementation.
    async fn complete(
        &self,
        pick: ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError>;

    /// Where session titles come from: [`Titles::Model`] only when a model
    /// is bound to the `title` role.
    fn titles(&self) -> Titles {
        Titles::First
    }

    /// What compaction needs to know about running on `pick`.
    fn budget(&self, pick: ModelPick<'_>) -> Budget {
        let _ = pick;
        Budget::default()
    }
}

/// The window a model call runs in, and how paths compact in it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    /// Input window, tokens; `None` when nothing knows it.
    pub context: Option<u64>,
    /// The most tokens one reply may take.
    pub output: Option<u64>,
    pub settings: CompactSettings,
    /// A model is bound to the `compact` role.
    pub compact_role: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classifier must decide on the strings genai/reqwest actually
    /// render, so the fixtures below are their `Display` shapes (genai 0.7
    /// flattens the whole error chain into one message).
    #[test]
    fn transient_statuses_and_network_faults() {
        // genai `WebModelCall` wrapping a `ResponseFailedStatus`.
        let t = |m: &str| LlmError::Request(m.to_owned()).is_transient();
        assert!(t(
            "Web call failed for model 'anthropic/m'.\nCause: Request failed with status code \
             '529'. Response body:\n{\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\"}}"
        ));
        assert!(t(
            "Web call failed for model 'anthropic/m'.\nCause: Request failed with status code \
             '429'. Response body:\nrate limited"
        ));
        // genai `HttpError`.
        assert!(t(
            "HTTP error.\nStatus: 503 Service Unavailable\nBody:\ntry again"
        ));
        assert!(LlmError::Stream(
            "Web stream error for model 'anthropic/m'.\nCause: model response body read timed out"
                .into()
        )
        .is_transient());
        // reqwest connect/send failures (cause chain not in the string) and
        // reqwest's own status display.
        assert!(t(
            "Web call failed for model 'anthropic/m'.\nCause: Reqwest error: error sending \
             request for url (https://api.example.com/v1/messages)"
        ));
        assert!(t(
            "Reqwest error: HTTP status server error (502) for url (https://x)"
        ));
    }

    #[test]
    fn overflow_errors() {
        let o = |m: &str| LlmError::Request(m.to_owned()).is_overflow();
        assert!(o(
            "HTTP error.\nStatus: 400 Bad Request\nBody:\n{\"error\":{\"type\":\
             \"invalid_request_error\",\"message\":\"prompt is too long: 210000 tokens > 200000 maximum\"}}"
        ));
        assert!(o(
            "HTTP error.\nStatus: 400 Bad Request\nBody:\n{\"error\":{\"code\":\
             \"context_length_exceeded\"}}"
        ));
        assert!(o("HTTP error.\nStatus: 413 Payload Too Large\nBody:\n"));
        assert!(!o(
            "HTTP error.\nStatus: 429 Too Many Requests\nBody:\ncontext window tokens per minute"
        ));
        assert!(!o(
            "HTTP error.\nStatus: 400 Bad Request\nBody:\nbad tool schema"
        ));
        assert!(!LlmError::MissingEnd.is_overflow());
    }

    #[test]
    fn non_transient_failures_stay_terminal() {
        let t = |m: &str| LlmError::Request(m.to_owned()).is_transient();
        // A 400 about anything (including a body that mentions retryable-looking
        // numbers) is decided by its status, which precedes the body.
        assert!(!t("HTTP error.\nStatus: 400 Bad Request\nBody:\n{\
             \"message\":\"max_tokens: `error (500)` is not valid\"}"));
        assert!(!t(
            "Web call failed for model 'anthropic/m'.\nCause: Request failed with status code \
             '401'. Response body:\nunauthorized"
        ));
        assert!(!t(
            "Web call failed for model 'anthropic/m'.\nCause: Request failed with status code \
             '404'. Response body:\nno such model"
        ));
        // Stream parse failures carry no status and match no network fragment.
        assert!(
            !LlmError::Stream(
                "Failed to parse stream data for model 'anthropic/m'.\nCause: expected value"
                    .into()
            )
            .is_transient()
        );
        assert!(!LlmError::MissingModel.is_transient());
        assert!(!LlmError::UnknownModel("rumu/cheap".into()).is_transient());
        assert!(!LlmError::Client("bad proxy url".into()).is_transient());
        // The provider cut the stream without concluding it: replayable.
        assert!(LlmError::MissingEnd.is_transient());
    }
}
