//! rumu-llm: the genai adapter. Projection from a branch path
//! and frozen system/tools into a genai `ChatRequest`, with Anthropic prompt
//! caching breakpoints, streaming deltas and usage capture.
//!
//! Model names like `vendor/model-id` cannot be mapped to an adapter by
//! name, so a `ServiceTargetResolver` forces `AdapterKind::Anthropic`, the
//! endpoint `$ANTHROPIC_BASE_URL/v1/` and the auth token from
//! `$ANTHROPIC_AUTH_TOKEN` (sent by genai's anthropic adapter as `x-api-key`).
//! The token is never logged.
//!
//! Reasoning is always requested explicitly (adaptive thinking at
//! `$RUMU_EFFORT`), with replayed thinking blocks that no longer verify
//! dropped by the provider (`block_binding: drop_block`) instead of failing
//! the request.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{
    Binary, CacheControl, ChatMessage, ChatOptions, ChatRole, ChatStreamEvent, ContentPart,
    CustomPart, MessageContent, MessageOptions, ReasoningEffort, ToolCall as GenaiToolCall,
    ToolResponse,
};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};
use rumu_blob::Blobs;
use rumu_core::{
    Attachment, Content, Node, NodeBody, ReasoningBlock, Segment, ToolCall, ToolSpec, Usage,
    image_label,
};
use rumu_kernel::{Delta, Llm, LlmError, LlmResponse, OnDelta};
use serde_json::{Value, json};
use tracing::warn;

/// Reasoning effort when `RUMU_EFFORT` is unset.
const DEFAULT_EFFORT: &str = "high";
/// Accepted `RUMU_EFFORT` values (Anthropic `output_config.effort`).
const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
/// The beta that enables `thinking.block_binding`.
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

pub struct GenaiLlm {
    client: Client,
    model: String,
    effort: String,
    /// Where the projection reads attachment bytes from; `None` projects
    /// every image as its stand-in text.
    blobs: Option<Blobs>,
}

impl GenaiLlm {
    /// Read attachments from this blob store (`<RUMU_HOME>/blobs`).
    pub fn with_blobs(mut self, blobs: Blobs) -> Self {
        self.blobs = Some(blobs);
        self
    }

    /// Build from `RUMU_MODEL` (required), `RUMU_EFFORT` (default `high`),
    /// `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN`.
    pub fn from_env() -> Result<Self, LlmError> {
        let model = std::env::var("RUMU_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .ok_or(LlmError::MissingModel)?;
        let effort = std::env::var("RUMU_EFFORT")
            .ok()
            .filter(|e| !e.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_EFFORT.to_owned());
        let base = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
        let token = std::env::var("ANTHROPIC_AUTH_TOKEN").unwrap_or_default();
        if base.is_empty() || token.is_empty() {
            warn!("ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN not set; LLM calls will fail");
        }
        let endpoint = if base.is_empty() {
            "https://api.anthropic.com/v1/".to_owned()
        } else {
            format!("{}/v1/", base.trim_end_matches('/'))
        };
        Self::with_target(model, effort, endpoint, token)
    }

    /// Build with an explicit model / effort / endpoint / token (tests, tools).
    pub fn with_target(
        model: String,
        effort: String,
        endpoint: String,
        token: String,
    ) -> Result<Self, LlmError> {
        if !EFFORTS.contains(&effort.as_str()) {
            return Err(LlmError::InvalidEffort(effort));
        }
        let resolver_model = model.clone();
        let resolver = move |mut t: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
            if token.is_empty() {
                return Err("ANTHROPIC_AUTH_TOKEN is not set".to_owned().into());
            }
            t.model = ModelIden::new(AdapterKind::Anthropic, resolver_model.as_str());
            t.endpoint = Endpoint::from_owned(endpoint.clone());
            t.auth = AuthData::from_single(token.clone());
            Ok(t)
        };
        let client = Client::builder()
            .with_service_target_resolver_fn(resolver)
            .build()
            .map_err(|e| LlmError::Client(e.to_string()))?;
        Ok(Self {
            client,
            model,
            effort,
            blobs: None,
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// One streamed completion. A failure carries whether any delta reached
    /// `on_delta` first (only an untouched request is safe to retry).
    async fn stream(
        &self,
        mut req: genai::chat::ChatRequest,
        on_delta: Option<OnDelta>,
    ) -> Result<LlmResponse, (LlmError, bool)> {
        // Static-prefix breakpoint: cache_control on the last tool (tools +
        // system prefix). genai's request-level `cache_control` defers to any
        // message-level breakpoint, so the tool-level one is how the prefix
        // breakpoint is expressed when tail breakpoints are also present.
        if let Some(last) = req.tools.as_mut().and_then(|t| t.last_mut()) {
            last.cache_control = Some(CacheControl::Ephemeral);
        }

        // Replayed thinking the provider dropped (`input_transformations`
        // on `message_start`), counted off the raw frames.
        let dropped = Arc::new(AtomicUsize::new(0));
        let counter = dropped.clone();
        let opts = ChatOptions::default()
            .with_capture_usage(true)
            .with_capture_tool_calls(true)
            .with_capture_reasoning_content(true)
            .with_stream_read_timeout(std::time::Duration::from_secs(120))
            // Any explicit effort stops genai from inferring one from a
            // model-name suffix (`…-high`). `Zero` adds nothing of its own but
            // a `thinking: disabled`, which `extra_body` replaces.
            .with_reasoning_effort(ReasoningEffort::Zero)
            .with_extra_body(reasoning_body(&self.effort))
            .with_extra_headers([("anthropic-beta", THINKING_BINDING_BETA)])
            .with_raw_frame_fn(move |_, frame| {
                if frame.data.contains("input_transformations")
                    && let Some(v) = frame.json()
                {
                    counter.fetch_add(dropped_thinking(&v), Ordering::Relaxed);
                }
            });

        let target = ModelIden::new(AdapterKind::Anthropic, self.model.as_str());
        let res = self
            .client
            .exec_chat_stream(target, req, Some(&opts))
            .await
            .map_err(|e| (LlmError::Request(e.to_string()), false))?;

        let mut text = String::new();
        let mut reasoning = String::new();
        // Thinking spans the first reasoning delta until the first text
        // delta (or the stream's end when the model never answers in text).
        let mut reason_start: Option<std::time::Instant> = None;
        let mut reason_ms: Option<u64> = None;
        let mut end: Option<genai::chat::StreamEnd> = None;
        let mut stream = res.stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(|e| {
                let streamed = !text.is_empty() || !reasoning.is_empty();
                (LlmError::Stream(e.to_string()), streamed)
            })?;
            match ev {
                ChatStreamEvent::Chunk(c) => {
                    if reason_ms.is_none()
                        && let Some(t0) = reason_start.take()
                    {
                        reason_ms = Some(t0.elapsed().as_millis() as u64);
                    }
                    text.push_str(&c.content);
                    if let Some(cb) = &on_delta {
                        cb(Delta::Text(c.content));
                    }
                }
                ChatStreamEvent::ReasoningChunk(c) => {
                    reason_start.get_or_insert_with(std::time::Instant::now);
                    reasoning.push_str(&c.content);
                    if let Some(cb) = &on_delta {
                        cb(Delta::Reasoning(c.content));
                    }
                }
                ChatStreamEvent::End(e) => end = Some(e),
                _ => {}
            }
        }
        let end = end.ok_or((LlmError::MissingEnd, true))?;
        if reason_ms.is_none()
            && let Some(t0) = reason_start
        {
            reason_ms = Some(t0.elapsed().as_millis() as u64);
        }
        let reasoning = reasoning_blocks(&end, reasoning);

        let tool_calls: Vec<ToolCall> = end
            .captured_tool_calls()
            .map(|cs| {
                cs.iter()
                    .map(|c| ToolCall {
                        call_id: c.call_id.clone(),
                        name: c.fn_name.clone(),
                        args: c.fn_arguments.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let usage = end
            .captured_usage
            .map(|u| {
                let details = u.prompt_tokens_details.as_ref();
                Usage {
                    prompt_tokens: u.prompt_tokens.unwrap_or_default().max(0) as u64,
                    completion_tokens: u.completion_tokens.unwrap_or_default().max(0) as u64,
                    cached_tokens: details
                        .and_then(|d| d.cached_tokens)
                        .unwrap_or_default()
                        .max(0) as u64,
                    cache_creation_tokens: details
                        .and_then(|d| d.cache_creation_tokens)
                        .unwrap_or_default()
                        .max(0) as u64,
                }
            })
            .unwrap_or_default();

        let dropped = dropped.load(Ordering::Relaxed);
        if dropped > 0 {
            warn!(
                dropped,
                "provider dropped replayed thinking blocks; earlier reasoning reset"
            );
        }
        Ok(LlmResponse {
            text,
            reasoning,
            reason_ms,
            reasoning_reset: dropped > 0,
            tool_calls,
            usage,
            model: self.model.clone(),
        })
    }
}

#[async_trait]
impl Llm for GenaiLlm {
    async fn complete(
        &self,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError> {
        let from = replay_from(nodes);
        let blobs = self.blobs.as_ref();
        match self
            .stream(
                project_from(system, tools, nodes, from, blobs),
                on_delta.clone(),
            )
            .await
        {
            Ok(resp) => Ok(resp),
            // Rejected before any output because of replayed reasoning the
            // provider cannot drop (a corrupted or foreign block, a modified
            // one): retry once without any, and record the reset.
            Err((e, false)) if rejects_reasoning(&e) && replays_reasoning(&nodes[from..]) => {
                warn!(error = %e, "provider rejected replayed reasoning; retrying without it");
                let req = project_from(system, tools, nodes, nodes.len(), blobs);
                let mut resp = self.stream(req, on_delta).await.map_err(|(e, _)| e)?;
                resp.reasoning_reset = true;
                Ok(resp)
            }
            Err((e, _)) => Err(e),
        }
    }
}

/// The reasoning part of every request, merged over genai's payload (which
/// replaces top-level keys whole): adaptive thinking with summarized text,
/// the effort, and replayed blocks that fail the prefix check dropped rather
/// than rejected.
fn reasoning_body(effort: &str) -> Value {
    json!({
        "thinking": {
            "type": "adaptive",
            "display": "summarized",
            "block_binding": { "prefix_mismatch_behavior": "drop_block" },
        },
        "output_config": { "effort": effort },
    })
}

/// Thinking blocks a `message_start` frame reports as dropped. A model
/// that cannot read another model's blocks (`model_binding_mismatch`) is a
/// routing choice, not lost reasoning: those return when the model does.
fn dropped_thinking(frame: &Value) -> usize {
    frame
        .pointer("/message/input_transformations")
        .and_then(Value::as_array)
        .map(|ts| {
            ts.iter()
                .filter(|t| t["type"] == "thinking_dropped")
                .filter(|t| t["reason"] != "model_binding_mismatch")
                .count()
        })
        .unwrap_or(0)
}

/// A 400 about replayed thinking (`Invalid signature in thinking block`,
/// `Invalid data in redacted_thinking block`, `cannot be modified`).
fn rejects_reasoning(e: &LlmError) -> bool {
    match e {
        LlmError::Request(m) | LlmError::Stream(m) => m.contains("400") && m.contains("thinking"),
        _ => false,
    }
}

/// Whether projecting `nodes` replays any thinking block.
fn replays_reasoning(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        matches!(&n.body, NodeBody::Assistant { reasoning, .. } if reasoning.iter().any(|b| {
            !matches!(b, ReasoningBlock::Thinking { signature: None, .. })
        }))
    })
}

/// Index of the latest reasoning reset on the path: reasoning before it is
/// never replayed (dropping a leading run of thinking blocks keeps the rest
/// valid; putting one back would not).
fn replay_from(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .rposition(|n| {
            matches!(
                n.body,
                NodeBody::Assistant {
                    reasoning_reset: true,
                    ..
                }
            )
        })
        .unwrap_or(0)
}

/// The thinking blocks genai captured, in the reply's original order: a
/// `ThoughtSignature` and the `ReasoningContent` after it are one signed block,
/// a `redacted_thinking` custom part is an opaque one. Without captured blocks
/// the streamed text is kept for display only (unsigned).
///
/// This covers the two shapes genai produces for leading reasoning blocks —
/// Anthropic's `thinking`/`redacted_thinking` and OpenAI Responses' encrypted
/// reasoning. A provider that attaches a signature to each part instead
/// (Gemini) is out of scope until an adapter for it is wired.
fn reasoning_blocks(end: &genai::chat::StreamEnd, text: String) -> Vec<ReasoningBlock> {
    let mut blocks = Vec::new();
    let parts: &[ContentPart] = end
        .captured_content
        .as_ref()
        .map(|content| content.parts().as_slice())
        .unwrap_or_default();
    let mut signature: Option<String> = None;
    for part in parts {
        match part {
            ContentPart::ThoughtSignature(sig) => {
                // Consecutive signatures are separate blocks (OpenAI encrypts
                // each reasoning item on its own), not one replaced signature.
                if let Some(previous) = signature.replace(sig.clone()) {
                    blocks.push(ReasoningBlock::Thinking {
                        text: String::new(),
                        signature: Some(previous),
                    });
                }
            }
            ContentPart::ReasoningContent(t) => blocks.push(ReasoningBlock::Thinking {
                text: t.clone(),
                signature: signature.take(),
            }),
            ContentPart::Custom(custom) if custom.typ() == Some("redacted_thinking") => {
                blocks.push(ReasoningBlock::Redacted {
                    data: custom.data().clone(),
                });
            }
            _ => {}
        }
    }
    // A signature the provider gave without text is still a replayable block.
    if let Some(signature) = signature {
        blocks.push(ReasoningBlock::Thinking {
            text: String::new(),
            signature: Some(signature),
        });
    }
    if blocks.is_empty() && !text.is_empty() {
        blocks.push(ReasoningBlock::Thinking {
            text,
            signature: None,
        });
    }
    blocks
}

/// Deterministically project a branch path into a genai `ChatRequest`
/// (the cache contract).
///
/// - User → user message following `content.segments()`: text, and each
///   attachment as its `Image n:` label followed by a base64 image part (its
///   blob read through `blobs`; a blob that cannot be read projects as the
///   attachment's placeholder plus an "(image unavailable)" note instead of
///   failing the request). Text-only content projects exactly as one text
///   message;
/// - Assistant → assistant text + tool calls, prefixed by its captured
///   thinking blocks so Anthropic can continue a tool-use loop (none before
///   the latest reasoning reset);
/// - ToolResult → tool response (even when its text is empty); a result with
///   attachments becomes one Custom part holding the full Anthropic
///   `tool_result` block, so it still batches with the results of its tool
///   batch into one message; the results of one batch share one message,
///   since providers that pair calls with results per message (Gemini) reject
///   them split;
/// - Note → `<note source="…">` user message, from `content.text` alone; a
///   note with empty text is a marker and projects nothing. Structured
///   `content.data` is never projected.
/// - Message-level ephemeral cache breakpoints on the **last message** and the
///   **last user-role message**.
pub fn project(
    system: &str,
    tools: &[ToolSpec],
    nodes: &[Node],
    blobs: Option<&Blobs>,
) -> genai::chat::ChatRequest {
    project_from(system, tools, nodes, replay_from(nodes), blobs)
}

/// [`project`], replaying thinking only from `nodes[replay_from..]`.
fn project_from(
    system: &str,
    tools: &[ToolSpec],
    nodes: &[Node],
    replay_from: usize,
    blobs: Option<&Blobs>,
) -> genai::chat::ChatRequest {
    let load = |a: &Attachment| -> Option<Vec<u8>> { blobs.and_then(|b| b.get(&a.hash).ok()) };
    let mut messages: Vec<ChatMessage> = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        match &n.body {
            NodeBody::User { content } => {
                if content.attachments.is_empty() {
                    messages.push(ChatMessage::user(content.text.clone()));
                    continue;
                }
                let parts: Vec<ContentPart> = pieces(content, &load)
                    .into_iter()
                    .map(|piece| match piece {
                        Piece::Text(t) => ContentPart::Text(t),
                        Piece::Image { mime, data, name } => {
                            ContentPart::Binary(Binary::from_base64(mime, data, name))
                        }
                    })
                    .collect();
                messages.push(ChatMessage::user(MessageContent::from_parts(parts)));
            }
            NodeBody::Assistant {
                content,
                reasoning,
                tool_calls,
                ..
            } => {
                let text = content.text.clone();
                let mut parts_out: Vec<ContentPart> = Vec::new();
                // Anthropic requires the thinking blocks of this turn — signed
                // or redacted — back, unchanged and ahead of text/tool_use, or
                // it rejects the tool-use follow-up.
                let replayed = if i < replay_from { &[][..] } else { reasoning };
                for block in replayed {
                    match block {
                        ReasoningBlock::Thinking {
                            text,
                            signature: Some(signature),
                        } => {
                            parts_out.push(ContentPart::ThoughtSignature(signature.clone()));
                            parts_out.push(ContentPart::ReasoningContent(text.clone()));
                        }
                        ReasoningBlock::Redacted { data } => {
                            parts_out.push(ContentPart::Custom(CustomPart {
                                model_iden: None,
                                data: data.clone(),
                            }));
                        }
                        // Unsigned thinking is display-only.
                        ReasoningBlock::Thinking {
                            signature: None, ..
                        } => {}
                    }
                }
                if !text.is_empty() {
                    parts_out.push(ContentPart::Text(text));
                }
                for c in tool_calls {
                    parts_out.push(ContentPart::ToolCall(GenaiToolCall {
                        call_id: c.call_id.clone(),
                        fn_name: c.name.clone(),
                        fn_arguments: c.args.clone(),
                        thought_signatures: None,
                    }));
                }
                if !parts_out.is_empty() {
                    messages.push(ChatMessage::assistant(MessageContent::from_parts(
                        parts_out,
                    )));
                }
            }
            NodeBody::ToolResult {
                call_id, content, ..
            } => {
                // A plain result is a ToolResponse (string content). One with
                // attachments needs Anthropic's block form — genai's adapter
                // passes Custom parts of a Tool message through verbatim, so
                // the whole `tool_result` block (text and image content
                // blocks in segments() order, labels included) still batches
                // with the sibling results into one message. `is_error` stays
                // unprojected, exactly as for the plain-string form.
                let response = if content.attachments.is_empty() {
                    ContentPart::ToolResponse(ToolResponse::new(
                        call_id.clone(),
                        content.text.clone(),
                    ))
                } else {
                    let blocks: Vec<Value> = pieces(content, &load)
                        .into_iter()
                        .map(|piece| match piece {
                            Piece::Text(t) => json!({ "type": "text", "text": t }),
                            Piece::Image { mime, data, .. } => json!({
                                "type": "image",
                                "source": { "type": "base64", "media_type": mime, "data": data },
                            }),
                        })
                        .collect();
                    ContentPart::Custom(CustomPart {
                        model_iden: None,
                        data: json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": blocks,
                        }),
                    })
                };
                // A batch's results are consecutive on the path (markers in
                // between project nothing).
                match messages.last_mut() {
                    Some(m) if m.role == ChatRole::Tool => m.content.push(response),
                    _ => messages.push(ChatMessage::tool(MessageContent::from_parts(vec![
                        response,
                    ]))),
                }
            }
            NodeBody::Note {
                source, content, ..
            } => {
                if content.text.is_empty() {
                    continue;
                }
                messages.push(ChatMessage::user(format!(
                    "<note source=\"{source}\">\n{}\n</note>",
                    content.text
                )));
            }
        }
    }

    if let Some(last) = messages.last_mut() {
        last.options = Some(MessageOptions::default().with_cache_control(CacheControl::Ephemeral));
    }
    if let Some(u) = messages.iter().rposition(|m| m.role == ChatRole::User)
        && Some(u) != messages.len().checked_sub(1)
    {
        messages[u].options =
            Some(MessageOptions::default().with_cache_control(CacheControl::Ephemeral));
    }

    let mut req = genai::chat::ChatRequest::from_system(system).append_messages(messages);
    if !tools.is_empty() {
        // Tools must arrive sorted by name (the kernel keeps them frozen and
        // sorted); a defensive sort keeps projection deterministic regardless.
        let mut tools: Vec<&ToolSpec> = tools.iter().collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        req = req.with_tools(tools.into_iter().map(|t| {
            genai::chat::Tool::new(t.name.clone())
                .with_description(t.description.clone())
                .with_schema(t.schema.clone())
        }));
    }
    req
}

/// One projected piece of a content with attachments, in `segments()` order.
enum Piece {
    Text(String),
    /// Base64 of the stored bytes, with the stored MIME type.
    Image {
        mime: String,
        data: String,
        name: Option<String>,
    },
}

/// Text and images of `content`, in reading order: empty text segments
/// dropped, each image preceded by its `Image n:` label. A blob that cannot
/// be read becomes `Image n: [image: name W×H] (image unavailable)` — a
/// missing image must never fail the request.
fn pieces(content: &Content, load: &dyn Fn(&Attachment) -> Option<Vec<u8>>) -> Vec<Piece> {
    let mut out = Vec::new();
    for segment in content.segments() {
        match segment {
            Segment::Text(t) if !t.is_empty() => out.push(Piece::Text(t.to_owned())),
            Segment::Text(_) => {}
            Segment::Image { n, attachment, .. } => match load(attachment) {
                Some(bytes) => {
                    out.push(Piece::Text(image_label(n)));
                    out.push(Piece::Image {
                        mime: attachment.mime.clone(),
                        data: base64::engine::general_purpose::STANDARD.encode(bytes),
                        name: attachment.name.clone(),
                    });
                }
                None => out.push(Piece::Text(format!(
                    "{} {} (image unavailable)",
                    image_label(n),
                    attachment.placeholder()
                ))),
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{NodeId, SessionId, now};
    use serde_json::json;

    fn node(body: NodeBody) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: SessionId::new(),
            at: now(),
            dur_ms: None,
            body,
        }
    }

    #[test]
    fn projection_shapes_and_breakpoints() {
        let nodes = vec![
            // An empty-text note is a marker: never projected.
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::WorkspaceUse { name: "x".into() }),
            }),
            node(NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content::text("remember A"),
            }),
            node(NodeBody::User {
                content: rumu_core::Content::text("hello"),
            }),
            node(NodeBody::Assistant {
                content: rumu_core::Content::text("calling"),
                reasoning: vec![ReasoningBlock::Thinking {
                    text: "hidden thinking".into(),
                    signature: None,
                }],
                reason_ms: None,
                reasoning_reset: false,
                tool_calls: vec![ToolCall {
                    call_id: "c1".into(),
                    name: "shell".into(),
                    args: json!({"command": "ls"}),
                }],
                model: "m".into(),
                usage: Usage::default(),
            }),
            // An empty ToolResult is still projected.
            node(NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text(""),
                is_error: false,
            }),
            node(NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content::text("remember B"),
            }),
        ];
        let tools = vec![
            ToolSpec {
                name: "zeta".into(),
                description: "z".into(),
                schema: json!({"type": "object"}),
                background: false,
            },
            ToolSpec {
                name: "alpha".into(),
                description: "a".into(),
                schema: json!({"type": "object"}),
                background: false,
            },
        ];
        let req = project("SYSTEM", &tools, &nodes, None);

        assert_eq!(req.system.as_deref(), Some("SYSTEM"));
        // Marker dropped; notes are user messages with the exact format.
        let m = &req.messages;
        assert_eq!(m.len(), 5);
        assert_eq!(
            m[0].content.first_text(),
            Some("<note source=\"memory\">\nremember A\n</note>")
        );
        assert_eq!(m[1].content.first_text(), Some("hello"));
        assert_eq!(m[2].role, ChatRole::Assistant);
        assert_eq!(m[2].content.tool_calls().len(), 1);
        assert_eq!(m[3].role, ChatRole::Tool);
        assert_eq!(
            m[4].content.first_text(),
            Some("<note source=\"memory\">\nremember B\n</note>")
        );

        // Breakpoints: the last message (the second note, also the last
        // user-role message) carries one; earlier notes must not.
        assert!(
            m[4].options
                .as_ref()
                .and_then(|o| o.cache_control.as_ref())
                .is_some()
        );
        assert!(m[0].options.is_none());
        // Tools sorted by name.
        let names: Vec<_> = req
            .tools
            .iter()
            .flat_map(|ts| ts.iter())
            .map(|t| t.name.as_str().to_owned())
            .collect();
        assert_eq!(names, vec!["alpha", "zeta"]);

        // Deterministic: identical input → byte-identical serialization.
        let a = serde_json::to_string(&project("SYSTEM", &tools, &nodes, None)).unwrap();
        let b = serde_json::to_string(&project("SYSTEM", &tools, &nodes, None)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn signed_reasoning_replays_before_text() {
        let nodes = vec![node(NodeBody::Assistant {
            content: rumu_core::Content::text("answer"),
            reasoning: vec![
                ReasoningBlock::Thinking {
                    text: "thought".into(),
                    signature: Some("sig".into()),
                },
                ReasoningBlock::Redacted {
                    data: json!({"type": "redacted_thinking", "data": "enc"}),
                },
            ],
            reason_ms: None,
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project("SYSTEM", &[], &nodes, None);
        let parts = req.messages[0].content.parts();
        // Thinking blocks lead, in the order they were captured: the signed
        // block, then the redacted one, then the text.
        assert!(matches!(&parts[0], ContentPart::ThoughtSignature(s) if s == "sig"));
        assert!(matches!(&parts[1], ContentPart::ReasoningContent(t) if t == "thought"));
        assert!(
            matches!(&parts[2], ContentPart::Custom(c) if c.typ() == Some("redacted_thinking"))
        );
        assert!(matches!(&parts[3], ContentPart::Text(t) if t == "answer"));
    }

    #[test]
    fn consecutive_signatures_stay_separate_blocks() {
        // OpenAI Responses encrypts each reasoning item on its own: bare,
        // consecutive signatures with no reasoning text between them.
        let end = genai::chat::StreamEnd {
            captured_usage: None,
            captured_stop_reason: None,
            captured_content: Some(MessageContent::from_parts(vec![
                ContentPart::ThoughtSignature("a".into()),
                ContentPart::ThoughtSignature("b".into()),
                ContentPart::Text("hi".into()),
            ])),
            captured_reasoning_content: None,
            captured_response_id: None,
        };
        let blocks = reasoning_blocks(&end, String::new());
        assert_eq!(
            blocks,
            vec![
                ReasoningBlock::Thinking {
                    text: String::new(),
                    signature: Some("a".into()),
                },
                ReasoningBlock::Thinking {
                    text: String::new(),
                    signature: Some("b".into()),
                },
            ]
        );
        // Each replays as its own signature part, ahead of the text.
        let nodes = vec![node(NodeBody::Assistant {
            content: rumu_core::Content::text("hi"),
            reasoning: blocks,
            reason_ms: None,
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project("SYSTEM", &[], &nodes, None);
        let signatures: Vec<&str> = req.messages[0]
            .content
            .parts()
            .iter()
            .filter_map(|part| match part {
                ContentPart::ThoughtSignature(s) => Some(s.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(signatures, ["a", "b"]);
    }

    #[test]
    fn unsigned_reasoning_is_display_only() {
        // A provider that returns plain reasoning with no signature: kept for
        // the UI, never replayed (Anthropic rejects an unsigned block).
        let end = genai::chat::StreamEnd {
            captured_usage: None,
            captured_stop_reason: None,
            captured_content: None,
            captured_reasoning_content: None,
            captured_response_id: None,
        };
        let blocks = reasoning_blocks(&end, "thought".into());
        assert_eq!(
            blocks,
            vec![ReasoningBlock::Thinking {
                text: "thought".into(),
                signature: None,
            }]
        );
        let nodes = vec![node(NodeBody::Assistant {
            content: rumu_core::Content::text("hi"),
            reasoning: blocks,
            reason_ms: None,
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project("SYSTEM", &[], &nodes, None);
        assert_eq!(req.messages[0].content.parts().len(), 1);
    }

    #[test]
    fn notes_project_only_their_frozen_text() {
        // The structured payload is UI-only; the model sees `content.text`.
        let payload = rumu_core::TurnError {
            message: "hidden payload".into(),
        };
        let frozen = "frozen text — [Bar](@rumu:/cards/foo/bar)";
        let nodes = vec![node(NodeBody::Note {
            source: "memory".into(),
            content: rumu_core::Content::text(frozen).with_data(&payload),
        })];
        let req = project("SYSTEM", &[], &nodes, None);
        assert_eq!(req.messages.len(), 1);
        let text = req.messages[0].content.first_text().unwrap();
        assert_eq!(text, format!("<note source=\"memory\">\n{frozen}\n</note>"));
        assert!(!text.contains("hidden payload"), "{text}");
    }

    // ---------- image attachments ----------

    /// A blob store holding `imgs`, plus the attachment naming each stored
    /// blob (hash and MIME are all the projection reads).
    fn blobs_with(imgs: &[&[u8]]) -> (tempfile::TempDir, Blobs, Vec<Attachment>) {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path()).unwrap();
        let attachments = imgs
            .iter()
            .map(|&bytes| {
                blobs.put(bytes).unwrap();
                Attachment {
                    hash: rumu_blob::hash_of(bytes),
                    mime: "image/png".into(),
                    name: Some(format!("{}.png", String::from_utf8_lossy(bytes))),
                    width: 4,
                    height: 3,
                    size: bytes.len() as u64,
                }
            })
            .collect();
        (dir, blobs, attachments)
    }

    /// The parts of one message as a compact shape: text verbatim, images as
    /// their base64 payload, everything else as `?`.
    fn shape(parts: &[ContentPart]) -> Vec<String> {
        parts
            .iter()
            .map(|p| match p {
                ContentPart::Text(t) => format!("text:{t:?}"),
                ContentPart::Binary(b) => match &b.source {
                    genai::chat::BinarySource::Base64(data) => format!("img:{data}"),
                    genai::chat::BinarySource::Url(u) => format!("url:{u}"),
                },
                _ => "?".to_owned(),
            })
            .collect()
    }

    #[test]
    fn user_images_project_at_their_anchors_with_labels() {
        let (_dir, blobs, atts) = blobs_with(&[b"aa", b"bb"]);
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);

        // Anchors place images inline, each behind its label, in segments()
        // order; unanchored images follow the text.
        let user = |text: &str| {
            vec![node(NodeBody::User {
                content: rumu_core::Content::text(text).with_attachments(atts.clone()),
            })]
        };
        let parts = |nodes: &[Node]| {
            project("SYSTEM", &[], nodes, Some(&blobs)).messages[0]
                .content
                .parts()
                .to_vec()
        };
        assert_eq!(
            shape(&parts(&user("before [Image #2] and [Image #1] after"))),
            [
                "text:\"before \"",
                "text:\"Image 2:\"",
                &format!("img:{}", b64(b"bb")),
                "text:\" and \"",
                "text:\"Image 1:\"",
                &format!("img:{}", b64(b"aa")),
                "text:\" after\"",
            ]
        );
        assert_eq!(
            shape(&parts(&user("look"))),
            [
                "text:\"look\"",
                "text:\"Image 1:\"",
                &format!("img:{}", b64(b"aa")),
                "text:\"Image 2:\"",
                &format!("img:{}", b64(b"bb")),
            ]
        );
        // Adjacent anchors leave no empty text parts.
        assert_eq!(
            shape(&parts(&user("[Image #1][Image #2]"))),
            [
                "text:\"Image 1:\"",
                &format!("img:{}", b64(b"aa")),
                "text:\"Image 2:\"",
                &format!("img:{}", b64(b"bb")),
            ]
        );

        // A text-only user message projects exactly as before, even with a
        // blob store configured (only the usual message-level breakpoint is
        // added by `project` itself).
        let plain = vec![node(NodeBody::User {
            content: rumu_core::Content::text("hello"),
        })];
        let msg = project("SYSTEM", &[], &plain, Some(&blobs)).messages[0].clone();
        assert_eq!(msg.role, ChatRole::User);
        assert_eq!(
            serde_json::to_value(&msg.content).unwrap(),
            serde_json::to_value(&ChatMessage::user("hello").content).unwrap()
        );
    }

    #[test]
    fn tool_result_with_images_is_one_anthropic_block() {
        let (_dir, blobs, atts) = blobs_with(&[b"pic"]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"pic");
        let nodes = vec![node(NodeBody::ToolResult {
            call_id: "c1".into(),
            content: rumu_core::Content::text("shot [Image #1]").with_attachments(atts),
            is_error: false,
        })];
        let m = &project("SYSTEM", &[], &nodes, Some(&blobs)).messages[0];
        assert_eq!(m.role, ChatRole::Tool);
        let parts = m.content.parts();
        assert_eq!(parts.len(), 1, "one Custom part holds the whole block");
        let ContentPart::Custom(c) = &parts[0] else {
            panic!("not a custom part: {parts:?}");
        };
        assert_eq!(
            c.data,
            json!({
                "type": "tool_result",
                "tool_use_id": "c1",
                "content": [
                    { "type": "text", "text": "shot " },
                    { "type": "text", "text": "Image 1:" },
                    { "type": "image", "source": {
                        "type": "base64", "media_type": "image/png", "data": b64,
                    }},
                ],
            })
        );
    }

    #[test]
    fn mixed_tool_results_batch_into_one_message() {
        let (_dir, blobs, atts) = blobs_with(&[b"pic"]);
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("go"),
            }),
            assistant(vec![], false, &["c1", "c2"]),
            node(NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text("plain"),
                is_error: false,
            }),
            node(NodeBody::ToolResult {
                call_id: "c2".into(),
                content: rumu_core::Content::text("shot").with_attachments(atts),
                is_error: false,
            }),
        ];
        let m = &project("SYSTEM", &[], &nodes, Some(&blobs)).messages;
        assert_eq!(m.len(), 3);
        assert_eq!(m[2].role, ChatRole::Tool);
        let parts = m[2].content.parts();
        assert_eq!(parts.len(), 2, "plain and image results share one message");
        assert!(matches!(&parts[0], ContentPart::ToolResponse(r) if r.call_id == "c1"));
        assert!(matches!(&parts[1], ContentPart::Custom(c) if c.data["tool_use_id"] == "c2"));
        // The message-level breakpoint still lands on this Tool message.
        assert!(
            m[2].options
                .as_ref()
                .and_then(|o| o.cache_control.as_ref())
                .is_some()
        );
    }

    #[test]
    fn a_missing_blob_projects_its_placeholder() {
        let gone = Attachment {
            hash: "f".repeat(64),
            mime: "image/png".into(),
            name: Some("gone.png".into()),
            width: 800,
            height: 600,
            size: 10,
        };
        let fallback = "Image 1: [image: gone.png 800×600] (image unavailable)";

        // In a user message …
        let nodes = vec![node(NodeBody::User {
            content: rumu_core::Content::text("see [Image #1]").with_attachments([gone.clone()]),
        })];
        let parts = project("SYSTEM", &[], &nodes, None).messages[0]
            .content
            .parts()
            .to_vec();
        assert_eq!(
            shape(&parts),
            ["text:\"see \"", &format!("text:{fallback:?}")]
        );

        // … and in a tool result.
        let nodes = vec![node(NodeBody::ToolResult {
            call_id: "c1".into(),
            content: rumu_core::Content::text("shot [Image #1]").with_attachments([gone]),
            is_error: false,
        })];
        let parts = project("SYSTEM", &[], &nodes, None).messages[0]
            .content
            .parts()
            .to_vec();
        let ContentPart::Custom(c) = &parts[0] else {
            panic!("not a custom part: {parts:?}");
        };
        assert_eq!(
            c.data["content"],
            json!([
                { "type": "text", "text": "shot " },
                { "type": "text", "text": fallback },
            ])
        );
    }

    #[test]
    fn image_projection_is_deterministic() {
        let (_dir, blobs, atts) = blobs_with(&[b"aa"]);
        let (_dir2, _blobs2, atts2) = blobs_with(&[b"bb"]);
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("see [Image #1]").with_attachments(atts),
            }),
            assistant(vec![], false, &["c1"]),
            node(NodeBody::ToolResult {
                call_id: "c1".into(),
                // The second image's blob is not in `blobs`: its fallback is
                // part of the determinism contract too.
                content: rumu_core::Content::text("shot [Image #1]").with_attachments(atts2),
                is_error: true,
            }),
        ];
        let a = serde_json::to_string(&project("SYSTEM", &[], &nodes, Some(&blobs))).unwrap();
        let b = serde_json::to_string(&project("SYSTEM", &[], &nodes, Some(&blobs))).unwrap();
        assert_eq!(a, b);
    }

    fn assistant(reasoning: Vec<ReasoningBlock>, reset: bool, calls: &[&str]) -> Node {
        node(NodeBody::Assistant {
            content: rumu_core::Content::text(""),
            reasoning,
            reason_ms: None,
            reasoning_reset: reset,
            tool_calls: calls
                .iter()
                .map(|id| ToolCall {
                    call_id: (*id).into(),
                    name: "shell".into(),
                    args: json!({}),
                })
                .collect(),
            model: "m".into(),
            usage: Usage::default(),
        })
    }

    fn result(call_id: &str) -> Node {
        node(NodeBody::ToolResult {
            call_id: call_id.into(),
            content: rumu_core::Content::text("ok"),
            is_error: false,
        })
    }

    fn signed(signature: &str) -> ReasoningBlock {
        ReasoningBlock::Thinking {
            text: "t".into(),
            signature: Some(signature.into()),
        }
    }

    #[test]
    fn one_batch_of_tool_results_is_one_message() {
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("go"),
            }),
            assistant(vec![], false, &["c1", "c2"]),
            result("c1"),
            // A marker inside the batch projects nothing and splits nothing.
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::WorkspaceUse { name: "x".into() }),
            }),
            result("c2"),
            node(NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content::text("after"),
            }),
            assistant(vec![], false, &["c3"]),
            result("c3"),
        ];
        let req = project("SYSTEM", &[], &nodes, None);
        let m = &req.messages;
        assert_eq!(m.len(), 6);
        let ids = |i: usize| -> Vec<String> {
            m[i].content
                .tool_responses()
                .iter()
                .map(|r| r.call_id.clone())
                .collect()
        };
        assert_eq!(m[2].role, ChatRole::Tool);
        assert_eq!(ids(2), ["c1", "c2"]);
        assert_eq!(m[3].role, ChatRole::User);
        assert_eq!(ids(5), ["c3"]);
    }

    #[test]
    fn reasoning_before_a_reset_is_not_replayed() {
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("go"),
            }),
            assistant(vec![signed("old")], false, &["c1"]),
            result("c1"),
            assistant(vec![signed("reset")], true, &["c2"]),
            result("c2"),
            assistant(vec![signed("new")], false, &[]),
        ];
        let signatures: Vec<String> = project("SYSTEM", &[], &nodes, None)
            .messages
            .iter()
            .flat_map(|m| m.content.thought_signatures())
            .map(str::to_owned)
            .collect();
        assert_eq!(signatures, ["reset", "new"]);
        assert!(replays_reasoning(&nodes[replay_from(&nodes)..]));
    }

    #[test]
    fn counts_dropped_thinking_but_not_model_switches() {
        let frame = json!({"type": "message_start", "message": {"input_transformations": [
            {"type": "thinking_dropped", "path": "messages.1.content.0", "reason": "prefix_binding_mismatch"},
            {"type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch"},
        ]}});
        assert_eq!(dropped_thinking(&frame), 1);
        assert_eq!(dropped_thinking(&json!({"type": "message_start"})), 0);
    }

    #[test]
    fn turn_error_marker_is_not_projected() {
        // A recorded turn failure sits at the branch head; the retry's
        // request must not mention it.
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("hello"),
            }),
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::TurnError {
                    message: "llm request failed: overloaded".into(),
                }),
            }),
        ];
        let req = project("SYSTEM", &[], &nodes, None);
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].content.first_text(), Some("hello"));
    }

    #[test]
    fn recognizes_reasoning_rejections() {
        let rejected = LlmError::Stream(
            "HTTP error. Status: 400 Bad Request Body: {\"message\":\"messages.1.content.0: \
             Invalid `signature` in `thinking` block\"}"
                .into(),
        );
        assert!(rejects_reasoning(&rejected));
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request: max_tokens".into()
        )));
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 529 thinking overloaded".into()
        )));
    }

    #[test]
    fn effort_is_validated() {
        let build = |effort: &str| {
            GenaiLlm::with_target("m".into(), effort.into(), "http://x/v1/".into(), "t".into())
        };
        assert!(matches!(build("extreme"), Err(LlmError::InvalidEffort(e)) if e == "extreme"));
        assert!(build("xhigh").is_ok());
    }
}
