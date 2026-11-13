//! rumu-llm: the genai adapter. Projection from a branch path and frozen
//! system/tools into a genai `ChatRequest`, with prompt caching
//! breakpoints, streaming deltas and usage capture.
//!
//! A provider has an endpoint per protocol it speaks (Anthropic, OpenAI chat
//! completions or Responses, Gemini), each with its own genai client whose
//! `ServiceTargetResolver` forces that protocol, API root and key: model ids
//! such as `vendor/model` cannot be mapped to an adapter by name. A model
//! runs on its native protocol when its provider has an endpoint for it,
//! else the first of [`Protocol::FALLBACK`] it has, unless the settings hold
//! it to one. The key is never logged. Which model a call runs on is
//! resolved per call from a [`ModelPick`] through the roles and custom
//! models of the [`ModelConfig`].
//!
//! On Anthropic, reasoning is always requested explicitly (adaptive thinking
//! at the resolved effort). Replayed thinking blocks whose prefix no longer
//! verifies are left to the provider's default: older accounts pass them
//! through (`thinking_mismatch_allowed`), so the model keeps its earlier
//! reasoning, and accounts that enforce the check reject the request, which
//! is retried once without any. We deliberately do not set
//! `block_binding: drop_block`: that would turn every prefix mismatch into
//! lost reasoning (and an `input_transformations` reset) even when the
//! provider would have accepted the block. History that rumu itself rewrites
//! (compaction) drops the affected thinking blocks in its projection instead.
//! Reasoning is only ever replayed to the model that produced it. Other
//! protocols do not report dropped input reasoning; a request they reject
//! because of it is retried once without any. Whichever way reasoning is left
//! out, the reply records it as a reset, so it is never replayed again and
//! the UI can say so.

pub mod catalog;
mod list;
pub mod presets;
mod probe;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

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
    Attachment, CUSTOM_PROVIDER, CallStats, CompactSettings, Content, CustomModel, Effort,
    ModelConfig, ModelInfo, ModelPick, Node, NodeBody, Protocol, ProviderSettings, ReasoningBlock,
    Role, Roles, Segment, Titles, ToolCall, ToolSpec, Usage, image_label, split_model_ref,
};
use rumu_kernel::{Budget, Delta, Llm, LlmError, LlmResponse, OnDelta};
use serde_json::{Value, json};
use tracing::warn;

pub use catalog::Catalog;
pub use list::list_models;
pub use probe::{Probe, probe};

/// The beta that reports `input_transformations` on `message_start`: blocks
/// the provider dropped, and blocks whose prefix did not verify but that it
/// passed through anyway (`thinking_mismatch_allowed`, which is not a reset).
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The genai adapter. Its providers, custom models and roles come from the
/// settings and can be replaced while running: a request uses the
/// configuration it started with.
#[derive(Default)]
pub struct GenaiLlm {
    config: RwLock<Arc<Resolved>>,
    /// Where the projection reads attachment bytes from; `None` projects
    /// every image as its stand-in text.
    blobs: Option<Blobs>,
}

#[derive(Default)]
struct Resolved {
    providers: BTreeMap<String, Provider>,
    custom: BTreeMap<String, CustomModel>,
    roles: Roles,
    titles: Titles,
    compact: CompactSettings,
    catalog: Arc<Catalog>,
}

struct Provider {
    settings: ProviderSettings,
    endpoints: BTreeMap<Protocol, String>,
    clients: BTreeMap<Protocol, Client>,
}

/// One call's resolved model.
struct Target {
    client: Client,
    protocol: Protocol,
    /// `provider/model-id`: stamped on the reply, and what reasoning is
    /// replayed to.
    model_ref: String,
    model_id: String,
    /// `None` when the model does not reason.
    effort: Option<Effort>,
    /// The catalog's output limit; `None` leaves genai's default.
    max_tokens: Option<u64>,
    /// The catalog's context window.
    context: Option<u64>,
    /// Whether requests carry the session identity ([`ProviderSettings`]).
    sticky: bool,
}

impl GenaiLlm {
    /// Read attachments from this blob store (`<RUMU_HOME>/blobs`).
    pub fn with_blobs(mut self, blobs: Blobs) -> Self {
        self.blobs = Some(blobs);
        self
    }

    /// Use `config` from the next request on. Refused (nothing changes)
    /// when a custom model routes to a provider that is not configured (or
    /// to a custom model), a role
    /// names a model of neither, or a model is held to a protocol its
    /// provider has no endpoint for.
    pub fn configure(&self, config: &ModelConfig, catalog: Arc<Catalog>) -> Result<(), LlmError> {
        let mut providers = BTreeMap::new();
        for (id, p) in &config.providers {
            let endpoints = endpoints(p);
            if let Some((m, _)) = p.protocols.iter().find(|(_, x)| !endpoints.contains_key(x)) {
                return Err(LlmError::NoEndpoint(format!("{id}/{m}")));
            }
            let clients = endpoints
                .iter()
                .map(|(&protocol, url)| Ok((protocol, client(protocol, url, p.api_key.clone())?)))
                .collect::<Result<_, LlmError>>()?;
            let provider = Provider {
                settings: p.clone(),
                endpoints,
                clients,
            };
            providers.insert(id.clone(), provider);
        }
        for (name, c) in &config.custom {
            let known = split_model_ref(&c.model).is_some_and(|(p, _)| providers.contains_key(p));
            if !known {
                return Err(LlmError::UnknownModel(format!(
                    "{} (custom model `{name}`)",
                    c.model
                )));
            }
        }
        for role in [Role::Default, Role::Title, Role::Compact] {
            if let Some(r) = config.roles.get(role) {
                let known = split_model_ref(r).is_some_and(|(p, m)| match p {
                    CUSTOM_PROVIDER => config.custom.contains_key(m),
                    _ => providers.contains_key(p),
                });
                if !known {
                    return Err(LlmError::UnknownModel(r.to_owned()));
                }
            }
        }
        *self.config.write().expect("llm config lock") = Arc::new(Resolved {
            providers,
            custom: config.custom.clone(),
            roles: config.roles.clone(),
            titles: config.titles,
            compact: config.compact,
            catalog,
        });
        Ok(())
    }

    fn resolved(&self) -> Arc<Resolved> {
        self.config.read().expect("llm config lock").clone()
    }
}

/// A provider's endpoints: its preset's, with its own over them (an empty
/// one turns the preset's off).
pub fn endpoints(p: &ProviderSettings) -> BTreeMap<Protocol, String> {
    let mut out: BTreeMap<Protocol, String> = p
        .preset
        .as_deref()
        .and_then(presets::preset)
        .map(|pre| {
            pre.endpoints
                .iter()
                .map(|(k, v)| (*k, (*v).to_owned()))
                .collect()
        })
        .unwrap_or_default();
    for (&protocol, url) in &p.endpoints {
        let url = url.trim().trim_end_matches('/');
        if url.is_empty() {
            out.remove(&protocol);
        } else {
            out.insert(protocol, url.to_owned());
        }
    }
    out
}

/// The protocol model `id` runs on: the one it is held to, else its native
/// one, else the first fallback; among those `endpoints` has.
pub fn protocol_for(
    catalog: &Catalog,
    p: &ProviderSettings,
    endpoints: &BTreeMap<Protocol, String>,
    id: &str,
) -> Option<Protocol> {
    if let Some(held) = p.protocols.get(id) {
        return endpoints.contains_key(held).then_some(*held);
    }
    catalog
        .native(id)
        .into_iter()
        .chain(Protocol::FALLBACK)
        .find(|x| endpoints.contains_key(x))
}

/// What is known about model `id` of provider `provider`.
pub fn model_info(
    catalog: &Catalog,
    provider: &str,
    p: &ProviderSettings,
    endpoints: &BTreeMap<Protocol, String>,
    id: &str,
) -> ModelInfo {
    let protocol = protocol_for(catalog, p, endpoints, id);
    let home = p.preset.as_deref().map(presets::home);
    let known = catalog.lookup(home, id);
    ModelInfo {
        model_ref: format!("{provider}/{id}"),
        id: id.to_owned(),
        name: catalog.name(home, id),
        protocol,
        native: catalog.native(id),
        pinned: p.pinned.iter().any(|x| x == id),
        efforts: match known {
            Some(m) => m.efforts.clone(),
            None => protocol.map(catalog::default_efforts).unwrap_or_default(),
        },
        context: known.and_then(|m| m.context),
        output: known.and_then(|m| m.output),
        image_input: known.is_none_or(|m| m.image_input),
        cost: known.and_then(|m| m.cost),
        known: known.is_some(),
        route: None,
    }
}

/// The URL paths are appended to: Anthropic endpoints are given without
/// their `/v1`.
pub(crate) fn api_root(protocol: Protocol, url: &str) -> String {
    let url = url.trim_end_matches('/');
    match protocol {
        Protocol::Anthropic => format!("{url}/v1/"),
        _ => format!("{url}/"),
    }
}

/// A client that sends every request to `url` with `key`, speaking
/// `protocol`.
fn client(protocol: Protocol, url: &str, key: String) -> Result<Client, LlmError> {
    let kind = adapter_kind(protocol);
    let endpoint = api_root(protocol, url);
    let resolver = move |mut t: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
        if key.is_empty() {
            return Err("no API key is set".to_owned().into());
        }
        t.model = ModelIden::new(kind, t.model.model_name.as_str());
        t.endpoint = Endpoint::from_owned(endpoint.clone());
        t.auth = AuthData::from_single(key.clone());
        Ok(t)
    };
    Client::builder()
        .with_service_target_resolver_fn(resolver)
        .build()
        .map_err(|e| LlmError::Client(e.to_string()))
}

fn adapter_kind(protocol: Protocol) -> AdapterKind {
    match protocol {
        Protocol::Anthropic => AdapterKind::Anthropic,
        Protocol::Openai => AdapterKind::OpenAI,
        Protocol::OpenaiResponses => AdapterKind::OpenAIResp,
        Protocol::Gemini => AdapterKind::Gemini,
    }
}

impl Resolved {
    /// The model `pick` names: `None` for a role no model is bound to (the
    /// default role must be bound).
    fn resolve(&self, pick: ModelPick<'_>) -> Result<Option<Target>, LlmError> {
        match pick {
            ModelPick::Role(role) => match self.roles.get(role) {
                Some(r) => self.model(r, None).map(Some),
                None if role == Role::Default => Err(LlmError::MissingModel),
                None => Ok(None),
            },
            ModelPick::Use(u) => self.model(&u.model, u.effort).map(Some),
        }
    }

    /// `model_ref` at `effort`; a custom one is the model it routes to, at
    /// `effort` over its own.
    fn model(&self, model_ref: &str, effort: Option<Effort>) -> Result<Target, LlmError> {
        match split_model_ref(model_ref) {
            Some((CUSTOM_PROVIDER, name)) => {
                let c = self
                    .custom
                    .get(name)
                    .ok_or_else(|| LlmError::UnknownModel(model_ref.to_owned()))?;
                self.provider_model(&c.model, effort.or(c.effort))
            }
            _ => self.provider_model(model_ref, effort),
        }
    }

    /// Configured provider model `model_ref` at `effort` (unset is high),
    /// moved to the nearest effort the model takes.
    fn provider_model(&self, model_ref: &str, effort: Option<Effort>) -> Result<Target, LlmError> {
        let unknown = || LlmError::UnknownModel(model_ref.to_owned());
        let (provider_id, model_id) = split_model_ref(model_ref).ok_or_else(unknown)?;
        let provider = self.providers.get(provider_id).ok_or_else(unknown)?;
        let info = model_info(
            &self.catalog,
            provider_id,
            &provider.settings,
            &provider.endpoints,
            model_id,
        );
        let protocol = info
            .protocol
            .ok_or_else(|| LlmError::NoEndpoint(model_ref.to_owned()))?;
        Ok(Target {
            client: provider.clients[&protocol].clone(),
            protocol,
            model_ref: model_ref.to_owned(),
            model_id: model_id.to_owned(),
            effort: effort.unwrap_or(Effort::High).nearest(&info.efforts),
            max_tokens: info.output,
            context: info.context,
            sticky: provider.settings.sticky,
        })
    }
}

impl Target {
    fn options(&self, dropped: Arc<AtomicUsize>, session: &str) -> ChatOptions {
        let opts = ChatOptions::default()
            .with_capture_usage(true)
            .with_capture_tool_calls(true)
            .with_capture_reasoning_content(true)
            .with_stream_read_timeout(std::time::Duration::from_secs(120));
        // Only the OpenAI adapters send it; Anthropic's identity goes in
        // `metadata` below.
        let opts = match self.sticky {
            true => opts.with_prompt_cache_key(session),
            false => opts,
        };
        match (self.protocol, self.effort) {
            (Protocol::Anthropic, effort) => {
                // Anthropic requires `max_tokens`; genai otherwise guesses it
                // from the model name, which a gateway prefix (`g/claude-…`)
                // defeats.
                let opts = match self.max_tokens.and_then(|n| u32::try_from(n).ok()) {
                    Some(n) => opts.with_max_tokens(n),
                    None => opts,
                };
                // Any explicit effort stops genai from inferring one from a
                // model-name suffix (`…-high`). `Zero` adds nothing of its own
                // but a `thinking: disabled`, which `extra_body` replaces.
                let opts = opts.with_reasoning_effort(ReasoningEffort::Zero);
                let effort = effort.filter(|e| *e != Effort::None);
                let mut extra = effort.map_or_else(|| json!({}), |e| reasoning_body(e.as_str()));
                if self.sticky {
                    extra["metadata"] = json!({ "user_id": user_id(session) });
                }
                let opts = opts.with_extra_body(extra);
                let opts = match effort {
                    Some(_) => opts.with_extra_headers([("anthropic-beta", THINKING_BINDING_BETA)]),
                    None => opts,
                };
                // Replayed thinking the provider dropped
                // (`input_transformations` on `message_start`), counted off
                // the raw frames. Mismatches the provider passed through
                // (`thinking_mismatch_allowed`) are not drops and are not
                // counted. Anthropic is the only protocol that reports this;
                // the others reject replayed reasoning at most (caught by the
                // retry in `complete`) or accept it silently.
                opts.with_raw_frame_fn(move |_, frame| {
                    if frame.data.contains("input_transformations")
                        && let Some(v) = frame.json()
                    {
                        dropped.fetch_add(dropped_thinking(&v), Ordering::Relaxed);
                    }
                })
            }
            (_, Some(effort)) => opts.with_reasoning_effort(genai_effort(effort)),
            (_, None) => opts,
        }
    }

    /// One streamed completion. A failure carries whether any delta reached
    /// `on_delta` first (only an untouched request is safe to retry).
    async fn stream(
        &self,
        mut req: genai::chat::ChatRequest,
        on_delta: Option<OnDelta>,
        session: &str,
    ) -> Result<LlmResponse, (LlmError, bool)> {
        // Static-prefix breakpoint: cache_control on the last tool (tools +
        // system prefix). genai's request-level `cache_control` defers to any
        // message-level breakpoint, so the tool-level one is how the prefix
        // breakpoint is expressed when tail breakpoints are also present.
        if let Some(last) = req.tools.as_mut().and_then(|t| t.last_mut()) {
            last.cache_control = Some(CacheControl::Ephemeral);
        }

        let dropped = Arc::new(AtomicUsize::new(0));
        let opts = self.options(dropped.clone(), session);
        let target = ModelIden::new(adapter_kind(self.protocol), self.model_id.as_str());
        let sent = std::time::Instant::now();
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
        let mut ttft_ms: Option<u64> = None;
        let mut end: Option<genai::chat::StreamEnd> = None;
        let mut stream = res.stream;
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(|e| {
                let streamed = !text.is_empty() || !reasoning.is_empty();
                (LlmError::Stream(e.to_string()), streamed)
            })?;
            if ttft_ms.is_none()
                && matches!(
                    ev,
                    ChatStreamEvent::Chunk(_)
                        | ChatStreamEvent::ReasoningChunk(_)
                        | ChatStreamEvent::ToolCallChunk(_)
                )
            {
                ttft_ms = Some(sent.elapsed().as_millis() as u64);
            }
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
                    .map(|c| {
                        ToolCall::new(c.call_id.clone(), c.fn_name.clone(), c.fn_arguments.clone())
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
            stats: CallStats {
                ttft_ms,
                reason_ms,
                retries: 0,
            },
            reasoning_reset: dropped > 0,
            tool_calls,
            usage,
            model: self.model_ref.clone(),
        })
    }
}

#[async_trait]
impl Llm for GenaiLlm {
    async fn complete(
        &self,
        pick: ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError> {
        let target = self
            .resolved()
            .resolve(pick)?
            .ok_or(LlmError::MissingModel)?;
        // What a sticky provider's gateway routes a conversation by; the
        // root node carries the session's id.
        let session = nodes
            .first()
            .map(|n| n.session.to_string())
            .unwrap_or_default();
        let to = Projection {
            protocol: target.protocol,
            model: &target.model_ref,
        };
        let from = replay_from(nodes);
        let withheld = withholds_reasoning(to, &nodes[from..]);
        let blobs = self.blobs.as_ref();
        match target
            .stream(
                project_from(to, system, tools, nodes, from, blobs),
                on_delta.clone(),
                &session,
            )
            .await
        {
            Ok(mut resp) => {
                if withheld {
                    warn!(
                        model = to.model,
                        "reasoning from another model is not replayed; earlier reasoning reset"
                    );
                    resp.reasoning_reset = true;
                }
                Ok(resp)
            }
            // Rejected before any output because of replayed reasoning the
            // provider cannot drop (a corrupted or foreign block, a modified
            // one): retry once without any, and record the reset.
            Err((e, false)) if rejects_reasoning(&e) && replays_reasoning(to, &nodes[from..]) => {
                warn!(error = %e, "provider rejected replayed reasoning; retrying without it");
                let req = project_from(to, system, tools, nodes, nodes.len(), blobs);
                let mut resp = target
                    .stream(req, on_delta, &session)
                    .await
                    .map_err(|(e, _)| e)?;
                resp.reasoning_reset = true;
                Ok(resp)
            }
            Err((e, _)) => Err(e),
        }
    }

    fn titles(&self) -> Titles {
        let r = self.resolved();
        match r.titles {
            Titles::Model if r.roles.title.is_none() => Titles::First,
            t => t,
        }
    }

    fn budget(&self, pick: ModelPick<'_>) -> Budget {
        let r = self.resolved();
        let target = r.resolve(pick).ok().flatten();
        Budget {
            context: target.as_ref().and_then(|t| t.context),
            output: target.as_ref().and_then(|t| t.max_tokens),
            settings: r.compact,
            compact_role: r.roles.compact.is_some(),
        }
    }
}

fn genai_effort(effort: Effort) -> ReasoningEffort {
    match effort {
        Effort::None => ReasoningEffort::Zero,
        Effort::Minimal => ReasoningEffort::Minimal,
        Effort::Low => ReasoningEffort::Low,
        Effort::Medium => ReasoningEffort::Medium,
        Effort::High => ReasoningEffort::High,
        Effort::XHigh => ReasoningEffort::XHigh,
        Effort::Max => ReasoningEffort::Max,
    }
}

/// The reasoning part of every request, merged over genai's payload (which
/// replaces top-level keys whole): adaptive thinking with summarized text and
/// the effort. No `block_binding`: a replayed block whose prefix no longer
/// verifies must stay a provider decision (pass through on older accounts,
/// 400 on accounts that enforce the check), never a request-side drop.
fn reasoning_body(effort: &str) -> Value {
    json!({
        "thinking": {
            "type": "adaptive",
            "display": "summarized",
        },
        "output_config": { "effort": effort },
    })
}

/// The per-run half of the sticky identity: a fresh random id every start,
/// so the pair of it and a session id names nothing beyond this run's
/// conversations.
fn device_id() -> &'static str {
    static DEVICE: OnceLock<String> = OnceLock::new();
    DEVICE.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Anthropic's `metadata.user_id` for a sticky provider: the JSON shape
/// Claude Code sends (and the account-pooling gateway documents) ...
/// `device_id`, `account_uuid` and `session_id`; the gateway parses its
/// `session_id` to keep one conversation on one upstream account. The
/// device id is per run (see [`device_id`]), so it names nothing beyond this
/// run's conversations.
fn user_id(session: &str) -> String {
    json!({
        "device_id": device_id(),
        "account_uuid": "",
        "session_id": session,
    })
    .to_string()
}

/// Thinking blocks a `message_start` frame reports as dropped. Anthropic is
/// the only protocol that reports this; a model that cannot read another
/// model's blocks (`model_binding_mismatch`) is a routing choice, not lost
/// reasoning: those return when the model does. A prefix mismatch the
/// provider merely flags (`thinking_mismatch_allowed`) is not a drop: the
/// block still reached the model.
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

/// A 400 about replayed reasoning that dropping it would fix: Anthropic's
/// `thinking` blocks (`Invalid signature in thinking block`, `Invalid data in
/// redacted_thinking block`, `cannot be modified`), Gemini's
/// `thought_signature` (providers write it both with an underscore and as
/// `thought signature`), OpenAI's `encrypted_content`, and OpenAI-compatible
/// endpoints rejecting the `reasoning_content` field.
fn rejects_reasoning(e: &LlmError) -> bool {
    let message = match e {
        LlmError::Request(m) | LlmError::Stream(m) => m,
        _ => return false,
    };
    if e.status() != Some(400) {
        return false;
    }
    let lower = message.to_ascii_lowercase();
    // The opposite complaint: the provider wants the reasoning and the
    // request did not carry it (DeepSeek). Dropping it is not the fix.
    if lower.contains("must be passed back")
        || (lower.contains("missing") && lower.contains("reasoning"))
    {
        return false;
    }
    [
        "thinking",
        "thought_signature",
        "thought signature",
        "encrypted_content",
        "encrypted content",
        "reasoning_content",
        "reasoning content",
        // OpenAI Responses: a replayed reasoning item without the item it
        // must be followed by.
        "reasoning item",
        "type 'reasoning'",
        "type \\\"reasoning\\\"",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

/// Whether `block` is replayed to the model that made it: signed thinking and
/// redacted blocks on every protocol, unsigned thinking only as OpenAI chat
/// reasoning text. Mirrors how `project` emits them.
fn replayable(to: Projection<'_>, block: &ReasoningBlock) -> bool {
    match block {
        ReasoningBlock::Thinking {
            signature: Some(_), ..
        }
        | ReasoningBlock::Redacted { .. } => true,
        ReasoningBlock::Thinking {
            text,
            signature: None,
        } => to.protocol == Protocol::Openai && !text.is_empty(),
    }
}

/// Whether `block` can only ever be replayed to the model that made it: a
/// signed or opaque provider block. Unsigned OpenAI chat text is not bound,
/// and some providers require it passed back, so it is never treated as lost.
fn model_bound(block: &ReasoningBlock) -> bool {
    matches!(
        block,
        ReasoningBlock::Thinking {
            signature: Some(_),
            ..
        } | ReasoningBlock::Redacted { .. }
    )
}

/// Whether projecting `nodes` replays any thinking block.
fn replays_reasoning(to: Projection<'_>, nodes: &[Node]) -> bool {
    nodes
        .iter()
        .any(|n| to.replayed(&n.body).iter().any(|b| replayable(to, b)))
}

/// Whether `nodes` hold reasoning bound to another model. The request cannot
/// send it (projection replays only to its author), and with another model's
/// turn now in the conversation it can never become replayable again:
/// recorded as a reset, like a provider drop.
fn withholds_reasoning(to: Projection<'_>, nodes: &[Node]) -> bool {
    nodes.iter().any(|n| match &n.body {
        NodeBody::Assistant {
            reasoning, model, ..
        } => model != to.model && reasoning.iter().any(model_bound),
        _ => false,
    })
}

/// Who a request is projected for.
#[derive(Debug, Clone, Copy)]
pub struct Projection<'a> {
    pub protocol: Protocol,
    /// The model ref: reasoning is replayed only to the model that made it
    /// (another model cannot verify, or read, its blocks).
    pub model: &'a str,
}

impl Projection<'_> {
    fn replayed<'n>(&self, body: &'n NodeBody) -> &'n [ReasoningBlock] {
        match body {
            NodeBody::Assistant {
                reasoning, model, ..
            } if model == self.model => reasoning,
            _ => &[],
        }
    }
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
/// `ThoughtSignature` and the `ReasoningContent` after it are one signed block
/// (Anthropic `thinking`, an OpenAI Responses encrypted item, a Gemini
/// signature); a `redacted_thinking` or OpenAI chat `reasoning.*` detail
/// custom part is an opaque one. When no block carries text, the streamed text
/// is kept for display on the first block, or on an unsigned one of its own.
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
            ContentPart::Custom(custom)
                if custom
                    .typ()
                    .is_some_and(|t| t == "redacted_thinking" || t.starts_with("reasoning.")) =>
            {
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
    if text.is_empty() || blocks_have_text(&blocks) {
        return blocks;
    }
    match blocks.first_mut() {
        Some(ReasoningBlock::Thinking { text: t, .. }) => *t = text,
        _ => blocks.insert(
            0,
            ReasoningBlock::Thinking {
                text,
                signature: None,
            },
        ),
    }
    blocks
}

fn blocks_have_text(blocks: &[ReasoningBlock]) -> bool {
    blocks
        .iter()
        .any(|b| matches!(b, ReasoningBlock::Thinking { text, .. } if !text.is_empty()))
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
    to: Projection<'_>,
    system: &str,
    tools: &[ToolSpec],
    nodes: &[Node],
    blobs: Option<&Blobs>,
) -> genai::chat::ChatRequest {
    project_from(to, system, tools, nodes, replay_from(nodes), blobs)
}

/// [`project`], replaying thinking only from `nodes[replay_from..]`.
fn project_from(
    to: Projection<'_>,
    system: &str,
    tools: &[ToolSpec],
    nodes: &[Node],
    replay_from: usize,
    blobs: Option<&Blobs>,
) -> genai::chat::ChatRequest {
    let load = |a: &Attachment| -> Option<Vec<u8>> { blobs.and_then(|b| b.get(&a.hash).ok()) };
    let mut messages: Vec<ChatMessage> = Vec::new();
    // Images of tool results, on protocols whose tool results are text only:
    // sent as one user message right after the batch's results.
    let mut deferred: Vec<ContentPart> = Vec::new();
    let flush = |messages: &mut Vec<ChatMessage>, deferred: &mut Vec<ContentPart>| {
        if !deferred.is_empty() {
            let parts = std::mem::take(deferred);
            messages.push(ChatMessage::user(MessageContent::from_parts(parts)));
        }
    };
    for (i, n) in nodes.iter().enumerate() {
        if !matches!(n.body, NodeBody::ToolResult { .. }) && projects(&n.body) {
            flush(&mut messages, &mut deferred);
        }
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
                tool_calls,
                ..
            } => {
                let text = content.text.clone();
                let mut parts_out: Vec<ContentPart> = Vec::new();
                // Anthropic requires the thinking blocks of this turn — signed
                // or redacted — back, unchanged and ahead of text/tool_use, or
                // it rejects the tool-use follow-up. Gemini wants its
                // signatures on the first call; OpenAI-compatible reasoners
                // (DeepSeek, Kimi) want the plain reasoning text back.
                let replayed = if i < replay_from {
                    &[][..]
                } else {
                    to.replayed(&n.body)
                };
                let mut call_signatures: Vec<String> = Vec::new();
                for block in replayed {
                    match block {
                        ReasoningBlock::Thinking {
                            text,
                            signature: Some(signature),
                        } => {
                            if to.protocol == Protocol::Gemini && !tool_calls.is_empty() {
                                call_signatures.push(signature.clone());
                                continue;
                            }
                            parts_out.push(ContentPart::ThoughtSignature(signature.clone()));
                            parts_out.push(ContentPart::ReasoningContent(text.clone()));
                        }
                        ReasoningBlock::Redacted { data } => {
                            parts_out.push(ContentPart::Custom(CustomPart {
                                model_iden: None,
                                data: data.clone(),
                            }));
                        }
                        ReasoningBlock::Thinking {
                            text,
                            signature: None,
                        } if to.protocol == Protocol::Openai && !text.is_empty() => {
                            parts_out.push(ContentPart::ReasoningContent(text.clone()));
                        }
                        // Otherwise unsigned thinking is display-only.
                        ReasoningBlock::Thinking {
                            signature: None, ..
                        } => {}
                    }
                }
                if !text.is_empty() {
                    parts_out.push(ContentPart::Text(text));
                }
                for (k, c) in tool_calls.iter().enumerate() {
                    let signatures = (k == 0 && !call_signatures.is_empty())
                        .then(|| std::mem::take(&mut call_signatures));
                    parts_out.push(ContentPart::ToolCall(GenaiToolCall {
                        call_id: c.call_id.clone(),
                        fn_name: c.name.clone(),
                        fn_arguments: c.args.clone(),
                        thought_signatures: signatures,
                    }));
                }
                if !parts_out.is_empty() {
                    messages.push(ChatMessage::assistant(MessageContent::from_parts(
                        parts_out,
                    )));
                }
            }
            NodeBody::ToolResult {
                call_id,
                content,
                invoke: None,
                ..
            } => {
                // A plain result is a ToolResponse (string content). One with
                // attachments needs Anthropic's block form — genai's adapter
                // passes Custom parts of a Tool message through verbatim, so
                // the whole `tool_result` block (text and image content
                // blocks in segments() order, labels included) still batches
                // with the sibling results into one message. `is_error` stays
                // unprojected, exactly as for the plain-string form. Other
                // protocols get the text, with the images deferred.
                let response = if content.attachments.is_empty() {
                    ContentPart::ToolResponse(ToolResponse::new(
                        call_id.clone(),
                        content.text.clone(),
                    ))
                } else if to.protocol != Protocol::Anthropic {
                    let mut text: Vec<String> = Vec::new();
                    for piece in pieces(content, &load) {
                        match piece {
                            Piece::Text(t) => text.push(t),
                            // An image always follows its `Image n:` label.
                            Piece::Image { mime, data, name } => {
                                let label = text.last().cloned().unwrap_or_default();
                                deferred.push(ContentPart::Text(label));
                                deferred.push(ContentPart::Binary(Binary::from_base64(
                                    mime, data, name,
                                )));
                            }
                        }
                    }
                    ContentPart::ToolResponse(ToolResponse::new(call_id.clone(), text.join("\n")))
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
            // A plugin invoke's result is trace-only: it has no model
            // tool_use to answer, so projecting it would be an orphan result.
            NodeBody::ToolResult {
                invoke: Some(_), ..
            } => {}
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
    flush(&mut messages, &mut deferred);

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

/// Whether a node projects to anything (markers do not).
fn projects(body: &NodeBody) -> bool {
    match body {
        NodeBody::Note { content, .. } => !content.text.is_empty(),
        NodeBody::ToolResult {
            invoke: Some(_), ..
        } => false,
        _ => true,
    }
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
    use rumu_core::{Invoke, NodeId, SessionId, now};
    use serde_json::json;

    const ANTHROPIC: Projection<'static> = Projection {
        protocol: Protocol::Anthropic,
        model: "m",
    };

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
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![ToolCall::new("c1", "shell", json!({"command": "ls"}))],
                model: "m".into(),
                usage: Usage::default(),
            }),
            // An empty ToolResult is still projected.
            node(NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text(""),
                is_error: false,
                invoke: None,
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
                output_schema: None,
                parallel: false,
            },
            ToolSpec {
                name: "alpha".into(),
                description: "a".into(),
                schema: json!({"type": "object"}),
                background: false,
                output_schema: None,
                parallel: false,
            },
        ];
        let req = project(ANTHROPIC, "SYSTEM", &tools, &nodes, None);

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
        let a = serde_json::to_string(&project(ANTHROPIC, "SYSTEM", &tools, &nodes, None)).unwrap();
        let b = serde_json::to_string(&project(ANTHROPIC, "SYSTEM", &tools, &nodes, None)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn invoke_results_are_not_projected() {
        let nodes = vec![
            assistant(vec![], false, &["c1"]),
            node(NodeBody::ToolResult {
                call_id: "c1.1".into(),
                content: rumu_core::Content::text("trace only"),
                is_error: false,
                invoke: Some(Invoke {
                    parent: "c1".into(),
                    name: "read".into(),
                    args: json!({ "path": "a.rs" }),
                    blocked: None,
                }),
            }),
            result("c1"),
        ];
        let m = &project(ANTHROPIC, "SYSTEM", &[], &nodes, None).messages;
        // The assistant call and the direct result only: the invoke result
        // would be an orphan tool_use_id with no matching call.
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].role, ChatRole::Tool);
        let responses = m[1].content.tool_responses();
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].call_id, "c1");
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
            stats: Default::default(),
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
            stats: Default::default(),
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
    fn chat_reasoning_details_replay_verbatim() {
        // OpenAI chat completions (through OpenRouter-style gateways) carry
        // encrypted reasoning only in `reasoning_details`; the streamed text,
        // when there is any, stays for display beside it.
        let detail = json!({"type": "reasoning.encrypted", "data": "gAAA", "id": "rs_1"});
        let end = genai::chat::StreamEnd {
            captured_usage: None,
            captured_stop_reason: None,
            captured_content: Some(MessageContent::from_parts(vec![
                ContentPart::Custom(CustomPart {
                    model_iden: None,
                    data: detail.clone(),
                }),
                ContentPart::Text("hi".into()),
            ])),
            captured_reasoning_content: None,
            captured_response_id: None,
        };
        let blocks = reasoning_blocks(&end, "thought".into());
        assert_eq!(
            blocks,
            vec![
                ReasoningBlock::Thinking {
                    text: "thought".into(),
                    signature: None,
                },
                ReasoningBlock::Redacted {
                    data: detail.clone()
                },
            ]
        );
        let nodes = vec![node(NodeBody::Assistant {
            content: rumu_core::Content::text("hi"),
            reasoning: blocks,
            stats: Default::default(),
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let to = Projection {
            protocol: Protocol::Openai,
            model: "m",
        };
        let req = project(to, "SYSTEM", &[], &nodes, None);
        let parts = req.messages[0].content.parts();
        assert!(matches!(&parts[0], ContentPart::ReasoningContent(t) if t == "thought"));
        assert!(matches!(&parts[1], ContentPart::Custom(c) if c.data == detail));
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
            stats: Default::default(),
            reasoning_reset: false,
            tool_calls: vec![],
            model: "m".into(),
            usage: Usage::default(),
        })];
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
                    kind: rumu_core::AttachmentKind::Image,
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
            project(ANTHROPIC, "SYSTEM", &[], nodes, Some(&blobs)).messages[0]
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
        let msg = project(ANTHROPIC, "SYSTEM", &[], &plain, Some(&blobs)).messages[0].clone();
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
            invoke: None,
        })];
        let m = &project(ANTHROPIC, "SYSTEM", &[], &nodes, Some(&blobs)).messages[0];
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
    fn tool_result_images_follow_the_batch_on_other_protocols() {
        let (_dir, blobs, atts) = blobs_with(&[b"pic"]);
        let nodes = vec![
            assistant(vec![], false, &["c1", "c2"]),
            node(NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text("shot [Image #1]").with_attachments(atts),
                is_error: false,
                invoke: None,
            }),
            result("c2"),
        ];
        let openai = Projection {
            protocol: Protocol::Openai,
            model: "m",
        };
        let m = &project(openai, "SYSTEM", &[], &nodes, Some(&blobs)).messages;
        assert_eq!(m.len(), 3);
        let responses = m[1].content.tool_responses();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].content, "shot \nImage 1:");
        assert_eq!(m[2].role, ChatRole::User);
        let parts = m[2].content.parts();
        assert!(matches!(&parts[0], ContentPart::Text(t) if t == "Image 1:"));
        assert!(matches!(&parts[1], ContentPart::Binary(_)));
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
                invoke: None,
            }),
            node(NodeBody::ToolResult {
                call_id: "c2".into(),
                content: rumu_core::Content::text("shot").with_attachments(atts),
                is_error: false,
                invoke: None,
            }),
        ];
        let m = &project(ANTHROPIC, "SYSTEM", &[], &nodes, Some(&blobs)).messages;
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
            kind: rumu_core::AttachmentKind::Image,
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
        let parts = project(ANTHROPIC, "SYSTEM", &[], &nodes, None).messages[0]
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
            invoke: None,
        })];
        let parts = project(ANTHROPIC, "SYSTEM", &[], &nodes, None).messages[0]
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
                invoke: None,
            }),
        ];
        let a = serde_json::to_string(&project(ANTHROPIC, "SYSTEM", &[], &nodes, Some(&blobs)))
            .unwrap();
        let b = serde_json::to_string(&project(ANTHROPIC, "SYSTEM", &[], &nodes, Some(&blobs)))
            .unwrap();
        assert_eq!(a, b);
    }

    fn assistant(reasoning: Vec<ReasoningBlock>, reset: bool, calls: &[&str]) -> Node {
        node(NodeBody::Assistant {
            content: rumu_core::Content::text(""),
            reasoning,
            stats: Default::default(),
            reasoning_reset: reset,
            tool_calls: calls
                .iter()
                .map(|id| ToolCall::new(*id, "shell", json!({})))
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
            invoke: None,
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
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
        let signatures: Vec<String> = project(ANTHROPIC, "SYSTEM", &[], &nodes, None)
            .messages
            .iter()
            .flat_map(|m| m.content.thought_signatures())
            .map(str::to_owned)
            .collect();
        assert_eq!(signatures, ["reset", "new"]);
        assert!(replays_reasoning(ANTHROPIC, &nodes[replay_from(&nodes)..]));
    }

    #[test]
    fn counts_dropped_thinking_but_not_model_switches() {
        let frame = json!({"type": "message_start", "message": {"input_transformations": [
            {"type": "thinking_dropped", "path": "messages.1.content.0", "reason": "prefix_binding_mismatch"},
            {"type": "thinking_dropped", "path": "messages.3.content.0", "reason": "model_binding_mismatch"},
            {"type": "thinking_mismatch_allowed", "path": "messages.5.content.0", "reason": "prefix_binding_mismatch"},
        ]}});
        assert_eq!(dropped_thinking(&frame), 1);
        assert_eq!(dropped_thinking(&json!({"type": "message_start"})), 0);
    }

    #[test]
    fn replayed_thinking_is_not_dropped_by_the_request() {
        let body = reasoning_body("high");
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["thinking"]["display"], "summarized");
        // A prefix mismatch stays the provider's decision: older accounts
        // pass the block through, enforcing ones reject it (retried without
        // any). Asking for `drop_block` would lose reasoning the provider
        // would otherwise have accepted.
        assert!(body["thinking"].get("block_binding").is_none());
        assert_eq!(body["output_config"]["effort"], "high");
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
        let req = project(ANTHROPIC, "SYSTEM", &[], &nodes, None);
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
        // Gemini (through ZenMux's Vertex endpoint), verbatim.
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request Bad Request\nBody: {\"error\":{\"code\":\"400\",\
             \"message\":\"Invalid value at 'contents[1].parts[0].thought_signature' \
             (TYPE_BYTES),  (request_id: x)\"}}"
                .into()
        )));
        // Gemini's other spelling: spaced and capitalized.
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\
             \"Thought signature is not valid\"}}"
                .into()
        )));
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\"Function call is \
             missing a thought_signature in functionCall parts.\"}}"
                .into()
        )));
        // OpenAI Responses' encrypted reasoning.
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"message\":\"Invalid 'encrypted_content'\"}".into()
        )));
        // An endpoint that refuses the replayed field.
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\
             \"reasoning_content: extra fields not permitted\"}}"
                .into()
        )));
        // OpenAI Responses: an orphaned replayed reasoning item.
        assert!(rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\"Item 'rs_1' of type \
             'reasoning' was provided without its required following item.\"}}"
                .into()
        )));
        // The opposite: DeepSeek wants it and the request lacks it. Retrying
        // without reasoning would make this worse, not better.
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\"The reasoning_content \
             in the thinking mode must be passed back to the API\"}}"
                .into()
        )));
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request\nBody: {\"error\":{\"message\":\"Missing \
             reasoning_content field in the assistant message at message index 2\"}}"
                .into()
        )));
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 400 Bad Request: max_tokens".into()
        )));
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 529 thinking overloaded".into()
        )));
        // A reasoning keyword in a non-400 body is not a reasoning rejection.
        assert!(!rejects_reasoning(&LlmError::Stream(
            "Status: 500 Internal Server Error: thinking backend down".into()
        )));
    }

    #[test]
    fn unsigned_openai_chat_reasoning_is_replayed() {
        // OpenAI-compatible reasoners (DeepSeek, Kimi) take plain reasoning
        // text back with no signature; that is a replay, not display only.
        let nodes = vec![assistant(
            vec![ReasoningBlock::Thinking {
                text: "thought".into(),
                signature: None,
            }],
            false,
            &[],
        )];
        let openai = Projection {
            protocol: Protocol::Openai,
            model: "m",
        };
        assert!(replays_reasoning(openai, &nodes));
        assert!(!replays_reasoning(ANTHROPIC, &nodes));
    }

    #[test]
    fn reasoning_withheld_from_another_model_is_flagged() {
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("go"),
            }),
            assistant(vec![signed("mine")], false, &[]),
        ];
        let mine = Projection {
            protocol: Protocol::Anthropic,
            model: "m",
        };
        assert!(!withholds_reasoning(mine, &nodes));
        // Asking another model leaves the signed block behind: the reply is
        // the point after which it is never replayed again.
        assert!(withholds_reasoning(
            Projection {
                protocol: Protocol::Anthropic,
                model: "other",
            },
            &nodes
        ));

        // Unsigned reasoning is not bound to a model: Anthropic never
        // replays it, and OpenAI chat providers may require it back, so a
        // model switch loses nothing that must be cut.
        let unsigned = vec![assistant(
            vec![ReasoningBlock::Thinking {
                text: "thought".into(),
                signature: None,
            }],
            false,
            &[],
        )];
        for protocol in [Protocol::Anthropic, Protocol::Openai] {
            assert!(!withholds_reasoning(
                Projection {
                    protocol,
                    model: "other",
                },
                &unsigned
            ));
        }
    }

    fn config() -> ModelConfig {
        let mut c = ModelConfig::default();
        c.providers.insert(
            "a".into(),
            ProviderSettings {
                endpoints: [(Protocol::Anthropic, "https://a.example".to_owned())].into(),
                api_key: "k".into(),
                models: vec!["m".into()],
                ..Default::default()
            },
        );
        c.custom.insert(
            "cheap".into(),
            CustomModel {
                model: "a/m".into(),
                effort: Some(Effort::Minimal),
            },
        );
        c
    }

    #[test]
    fn resolves_roles_custom_and_provider_models() {
        let llm = GenaiLlm::default();
        let mut c = config();
        llm.configure(&c, Arc::default()).unwrap();
        let r = llm.resolved();
        // No default role bound.
        assert!(matches!(
            r.resolve(ModelPick::Role(Role::Default)),
            Err(LlmError::MissingModel)
        ));
        assert!(r.resolve(ModelPick::Role(Role::Title)).unwrap().is_none());
        assert_eq!(llm.titles(), Titles::First);

        // A custom model runs on the model it routes to, at its effort moved
        // to the nearest the model takes; a marker's override wins.
        let pick = rumu_core::ModelUse {
            model: "rumu/cheap".into(),
            effort: None,
        };
        let t = r.resolve(ModelPick::Use(&pick)).unwrap().unwrap();
        assert_eq!((t.model_ref.as_str(), t.effort), ("a/m", Some(Effort::Low)));
        let pick = rumu_core::ModelUse {
            model: "a/custom".into(),
            effort: Some(Effort::Max),
        };
        let t = r.resolve(ModelPick::Use(&pick)).unwrap().unwrap();
        assert_eq!(
            (t.model_id.as_str(), t.effort),
            ("custom", Some(Effort::Max))
        );
        let gone = rumu_core::ModelUse {
            model: "rumu/gone".into(),
            effort: None,
        };
        assert!(matches!(
            r.resolve(ModelPick::Use(&gone)),
            Err(LlmError::UnknownModel(_))
        ));

        // A role names a custom model or a provider's.
        // Titles come from a model only while one is bound to the role.
        c.titles = Titles::Model;
        llm.configure(&c, Arc::default()).unwrap();
        assert_eq!(llm.titles(), Titles::First);
        c.roles.title = Some("rumu/cheap".into());
        c.roles.default = Some("a/m".into());
        llm.configure(&c, Arc::default()).unwrap();
        assert_eq!(llm.titles(), Titles::Model);
        let t = llm
            .resolved()
            .resolve(ModelPick::Role(Role::Default))
            .unwrap()
            .unwrap();
        assert_eq!(t.model_ref, "a/m");

        // Refused configurations leave the previous one in place.
        for bad in ["rumu/gone", "b/m"] {
            c.roles.default = Some(bad.into());
            assert!(llm.configure(&c, Arc::default()).is_err());
        }
        c.roles.default = None;
        c.custom.get_mut("cheap").unwrap().model = "b/m".into();
        assert!(matches!(
            llm.configure(&c, Arc::default()),
            Err(LlmError::UnknownModel(_))
        ));
        // A custom model never routes to another.
        c.custom.get_mut("cheap").unwrap().model = "rumu/cheap".into();
        assert!(llm.configure(&c, Arc::default()).is_err());
        assert_eq!(llm.titles(), Titles::Model);
    }

    #[test]
    fn models_run_on_their_native_protocol_then_the_fallbacks() {
        let catalog = Catalog::default();
        let mut p = ProviderSettings {
            preset: Some("openai".into()),
            ..Default::default()
        };
        let mut eps = endpoints(&p);
        assert_eq!(eps[&Protocol::OpenaiResponses], "https://api.openai.com/v1");
        let on = |p: &ProviderSettings, eps: &BTreeMap<Protocol, String>, id: &str| {
            protocol_for(&catalog, p, eps, id)
        };
        assert_eq!(on(&p, &eps, "gpt-9"), Some(Protocol::OpenaiResponses));
        assert_eq!(on(&p, &eps, "someone/else"), Some(Protocol::Openai));

        // An endpoint of its own; a native protocol the provider lacks.
        p.endpoints
            .insert(Protocol::Anthropic, " https://gw.example/ ".into());
        eps = endpoints(&p);
        assert_eq!(eps[&Protocol::Anthropic], "https://gw.example");
        assert_eq!(on(&p, &eps, "vendor/claude-9"), Some(Protocol::Anthropic));
        assert_eq!(on(&p, &eps, "gemini-9"), Some(Protocol::Openai));

        // An empty endpoint turns the preset's off.
        let mut off = p.clone();
        off.endpoints
            .insert(Protocol::OpenaiResponses, String::new());
        assert!(!endpoints(&off).contains_key(&Protocol::OpenaiResponses));

        // Held to one: only that, if there.
        p.protocols.insert("gpt-9".into(), Protocol::Anthropic);
        p.protocols.insert("gemini-9".into(), Protocol::Gemini);
        assert_eq!(on(&p, &eps, "gpt-9"), Some(Protocol::Anthropic));
        assert_eq!(on(&p, &eps, "gemini-9"), None);
        assert_eq!(
            api_root(Protocol::Anthropic, &eps[&Protocol::Anthropic]),
            "https://gw.example/v1/"
        );

        let mut c = ModelConfig::default();
        c.providers.insert("o".into(), p);
        assert!(matches!(
            GenaiLlm::default().configure(&c, Arc::default()),
            Err(LlmError::NoEndpoint(_))
        ));
    }

    #[test]
    fn reasoning_is_replayed_only_to_its_model() {
        let nodes = vec![
            node(NodeBody::User {
                content: rumu_core::Content::text("go"),
            }),
            assistant(vec![signed("mine")], false, &["c1"]),
            result("c1"),
        ];
        let sigs = |to: Projection<'_>| -> Vec<String> {
            project(to, "S", &[], &nodes, None)
                .messages
                .iter()
                .flat_map(|m| m.content.thought_signatures())
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(sigs(ANTHROPIC), ["mine"]);
        let other = Projection {
            protocol: Protocol::Anthropic,
            model: "other",
        };
        assert!(sigs(other).is_empty());
        assert!(!replays_reasoning(other, &nodes));

        // Gemini carries the signatures on the first call instead.
        let gemini = Projection {
            protocol: Protocol::Gemini,
            model: "m",
        };
        let req = project(gemini, "S", &[], &nodes, None);
        let calls = req.messages[1].content.tool_calls();
        assert_eq!(
            calls[0].thought_signatures.as_deref(),
            Some(&["mine".to_owned()][..])
        );
        assert!(req.messages[1].content.thought_signatures().is_empty());
    }

    #[test]
    fn sticky_user_id_names_the_session() {
        let id: Value = serde_json::from_str(&user_id("s1")).unwrap();
        assert!(id["device_id"].as_str().is_some_and(|d| !d.is_empty()));
        // The account-pooling gateway (and Claude Code) send this field; a
        // missing one can keep the request out of the gateway's cache keys.
        assert_eq!(id["account_uuid"], "");
        assert_eq!(id["session_id"], "s1");
    }
}
