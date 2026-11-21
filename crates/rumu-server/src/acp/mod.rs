//! `rumu acp`: rumu as an [Agent Client Protocol] agent over stdio, protocol
//! v1. It boots the whole stack in this process and drives it the way the
//! web does — the WebSocket methods and the push stream — so the translation
//! does not care whether rumu runs here or elsewhere.
//!
//! An ACP session is the main line of a rumu session. A `session/prompt`
//! answers once that line is idle: no turn running or waiting to start, and
//! no background sub-agent left in it. A background tool task does not hold
//! it (it may never end); the turn its completion wakes still comes out as
//! `session/update`s, which flow whether a prompt waits or not.
//!
//! [Agent Client Protocol]: https://agentclientprotocol.com

mod config;
mod decide;
mod events;
mod session;
mod v1;

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, CancelNotification, ContentBlock, Implementation, InitializeRequest,
    InitializeResponse, Meta, NewSessionRequest, NewSessionResponse, PromptCapabilities,
    PromptRequest, PromptResponse, SessionNotification, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::v1::{ConnectionTo, Responder};
use agent_client_protocol::{
    Agent, Client, Error, ErrorCode, JsonRpcRequest, JsonRpcResponse, Stdio,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::{boot, ws};
use config::{Choices, Config};
use session::{Outcome, Session, Stop};

/// The rumu stack, driven as a WebSocket client would.
#[derive(Clone)]
pub(super) struct Rumu {
    state: ws::AppState,
}

impl Rumu {
    /// Run one WebSocket method; an error is its message.
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        ws::call(&self.state, method, &params)
            .await
            .map_err(|(_, message, _)| message)
    }

    /// Every push from now on.
    fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.state.kernel.subscribe()
    }
}

/// `_session/steering`: send a message into the running turn.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteerResponse)]
#[serde(rename_all = "camelCase")]
struct SteerRequest {
    session_id: String,
    prompt: Vec<ContentBlock>,
    #[serde(default, rename = "_meta")]
    meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct SteerResponse {
    /// `injected`, `startedNewTurn` or `promptRequired`.
    outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// The agent's state across the connection.
struct Acp {
    rumu: Rumu,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    /// Whether the client takes form elicitations.
    elicit: AtomicBool,
    /// The session prompted last: a decision of a session this connection
    /// did not open (one a `session.create` started) is put to its client.
    last: Mutex<Option<String>>,
}

impl Acp {
    fn session(&self, id: &str) -> Result<Arc<Session>, Error> {
        self.sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| invalid(format!("unknown session `{id}`")))
    }

    fn touch(&self, id: &str) {
        *self.last.lock().unwrap() = Some(id.to_owned());
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::INVALID_PARAMS, message)
}

fn internal(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::INTERNAL_ERROR, message)
}

/// Serve ACP on stdin/stdout until the client hangs up.
pub async fn run(home: &Path) -> anyhow::Result<()> {
    let booted = boot::boot(home).await?;
    let rumu = Rumu {
        state: booted.state.clone(),
    };
    // Before anything can push: no push of a session is missed.
    let pushes = rumu.subscribe();
    let acp = Arc::new(Acp {
        rumu,
        sessions: Mutex::new(HashMap::new()),
        elicit: AtomicBool::new(false),
        last: Mutex::new(None),
    });
    Agent
        .v1()
        .name("rumu")
        .on_receive_request(
            {
                let acp = acp.clone();
                async move |req: InitializeRequest,
                            responder: Responder<InitializeResponse>,
                            _cx| {
                    let form = req
                        .client_capabilities
                        .elicitation
                        .as_ref()
                        .is_some_and(|e| e.form.is_some());
                    acp.elicit.store(form, Ordering::SeqCst);
                    responder.respond(initialize_response())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let acp = acp.clone();
                async move |req: NewSessionRequest,
                            responder: Responder<NewSessionResponse>,
                            _cx| {
                    match new_session(&acp, &req).await {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(e),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let acp = acp.clone();
                async move |req: PromptRequest,
                            responder: Responder<PromptResponse>,
                            cx: ConnectionTo<Client>| {
                    let session = match acp.session(&req.session_id.0) {
                        Ok(s) => s,
                        Err(e) => return responder.respond_with_error(e),
                    };
                    acp.touch(&session.id);
                    let text = v1::prompt_text(&req.prompt);
                    let acp = acp.clone();
                    cx.spawn(async move {
                        match session.prompt(&acp.rumu, text).await {
                            Ok(outcome) => responder.respond(prompt_response(outcome)),
                            Err(message) => responder.respond_with_error(internal(message)),
                        }
                    })
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let acp = acp.clone();
                async move |req: CancelNotification, cx: ConnectionTo<Client>| {
                    let Ok(session) = acp.session(&req.session_id.0) else {
                        return Ok(());
                    };
                    let rumu = acp.rumu.clone();
                    cx.spawn(async move {
                        if let Err(e) = session.cancel(&rumu).await {
                            tracing::warn!(error = %e, "cancel failed");
                        }
                        Ok(())
                    })
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let acp = acp.clone();
                async move |req: SetSessionConfigOptionRequest,
                            responder: Responder<SetSessionConfigOptionResponse>,
                            _cx| {
                    match set_config(&acp, &req).await {
                        Ok(options) => {
                            responder.respond(SetSessionConfigOptionResponse::new(options))
                        }
                        Err(e) => responder.respond_with_error(e),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let acp = acp.clone();
                async move |req: SteerRequest, responder: Responder<SteerResponse>, _cx| {
                    match steer(&acp, req).await {
                        Ok(response) => responder.respond(response),
                        Err(e) => responder.respond_with_error(e),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(Stdio::new(), async move |cx: ConnectionTo<Client>| {
            tokio::select! {
                () = pump(acp, cx.clone(), pushes) => Ok(()),
                () = cx.incoming_closed() => Ok(()),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("ACP connection failed: {e}"))?;
    drop(booted);
    Ok(())
}

fn initialize_response() -> InitializeResponse {
    let capabilities = AgentCapabilities::new()
        .prompt_capabilities(PromptCapabilities::new().embedded_context(true))
        // A `session/prompt` sent while one runs waits its turn.
        .meta(Meta::from_iter([(
            "promptQueueing".to_owned(),
            json!(true),
        )]));
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(capabilities)
        .agent_info(Implementation::new("rumu", env!("CARGO_PKG_VERSION")).title("rumu"))
        .meta(Meta::from_iter([(
            "steering".to_owned(),
            json!({ "supported": true }),
        )]))
}

async fn new_session(acp: &Acp, req: &NewSessionRequest) -> Result<NewSessionResponse, Error> {
    let choices = Choices::load(&acp.rumu).await.map_err(internal)?;
    let mut config = Config::default();
    choices.resolve(&mut config);
    let session = Session::create(&acp.rumu, &req.cwd, config.clone())
        .await
        .map_err(internal)?;
    let options = choices.options(&config);
    acp.sessions
        .lock()
        .unwrap()
        .insert(session.id.clone(), session.clone());
    Ok(NewSessionResponse::new(session.id.clone()).config_options(options))
}

async fn set_config(
    acp: &Acp,
    req: &SetSessionConfigOptionRequest,
) -> Result<Vec<agent_client_protocol::v1::schema::SessionConfigOption>, Error> {
    let session = acp.session(&req.session_id.0)?;
    let value = match &req.value {
        agent_client_protocol::v1::schema::SessionConfigOptionValue::ValueId { value } => {
            value.0.to_string()
        }
        _ => return Err(invalid("rumu's options are all selects")),
    };
    let choices = Choices::load(&acp.rumu).await.map_err(internal)?;
    let mut config = session.config.lock().unwrap().clone();
    let before = config.assistant.clone();
    choices
        .apply(&mut config, &req.config_id.0, &value)
        .map_err(invalid)?;
    if config.assistant != before {
        session
            .rebind(&acp.rumu, config.assistant.as_deref())
            .await
            .map_err(invalid)?;
    }
    *session.config.lock().unwrap() = config.clone();
    Ok(choices.options(&config))
}

async fn steer(acp: &Acp, req: SteerRequest) -> Result<SteerResponse, Error> {
    let session = acp.session(&req.session_id)?;
    let text = v1::prompt_text(&req.prompt);
    if text.trim().is_empty() {
        return Err(invalid("the steering prompt is empty"));
    }
    let busy = session.busy();
    let required = req
        .meta
        .as_ref()
        .and_then(|m| m.pointer("/steering/idleBehavior"))
        .and_then(Value::as_str)
        == Some("promptRequired");
    if !busy && required {
        return Ok(SteerResponse {
            outcome: "promptRequired".into(),
            reason: Some("noRunningTurn".into()),
        });
    }
    acp.touch(&session.id);
    session.steer(&acp.rumu, text).await.map_err(internal)?;
    Ok(SteerResponse {
        outcome: if busy { "injected" } else { "startedNewTurn" }.into(),
        reason: None,
    })
}

fn prompt_response(outcome: Outcome) -> PromptResponse {
    let stop = match outcome.stop {
        Stop::EndTurn => StopReason::EndTurn,
        Stop::Refusal => StopReason::Refusal,
        Stop::Cancelled => StopReason::Cancelled,
    };
    let u = outcome.usage;
    let usage = agent_client_protocol::v1::schema::Usage::new(
        u.prompt_tokens + u.completion_tokens,
        u.prompt_tokens,
        u.completion_tokens,
    )
    .cached_read_tokens(u.cached_tokens)
    .cached_write_tokens(u.cache_creation_tokens);
    PromptResponse::new(stop).usage(usage)
}

/// Translate rumu's pushes into `session/update`s, and put its questions to
/// the client, until the push stream ends.
async fn pump(acp: Arc<Acp>, cx: ConnectionTo<Client>, mut pushes: broadcast::Receiver<Value>) {
    loop {
        let push = match pushes.recv().await {
            Ok(push) => push,
            Err(RecvError::Lagged(n)) => {
                tracing::warn!(dropped = n, "ACP fell behind rumu's pushes");
                continue;
            }
            Err(RecvError::Closed) => return,
        };
        let method = push
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = push.get("params").unwrap_or(&Value::Null);
        let sessions: Vec<Arc<Session>> = acp.sessions.lock().unwrap().values().cloned().collect();
        if method == "decision.requested" {
            let of = params.get("session").and_then(Value::as_str);
            let last = acp.last.lock().unwrap().clone();
            let session = sessions
                .iter()
                .find(|s| Some(s.rumu_session().as_str()) == of)
                .or_else(|| sessions.iter().find(|s| Some(&s.id) == last.as_ref()));
            let (rumu, cx, params) = (acp.rumu.clone(), cx.clone(), params.clone());
            let elicit = acp.elicit.load(Ordering::SeqCst);
            let id = session.map(|s| s.id.clone());
            tokio::spawn(async move {
                match id {
                    // An editor's prompt cannot be withdrawn: no clock.
                    Some(id) => {
                        let _ = rumu
                            .call("decision.hold", json!({ "id": params.get("id") }))
                            .await;
                        decide::answer(rumu, cx, id, elicit, params).await
                    }
                    // No client session to ask: declined.
                    None => {
                        let _ = rumu
                            .call(
                                "decision.answer",
                                json!({ "id": params.get("id"), "answer": {} }),
                            )
                            .await;
                    }
                }
            });
            continue;
        }
        for session in sessions {
            let state = if session.needs_state(method, params) {
                acp.rumu
                    .call("branch.path", json!({ "branch": session.branch() }))
                    .await
                    .ok()
                    .and_then(|v| serde_json::from_value(v.get("state")?.clone()).ok())
            } else {
                None
            };
            for event in session.feed(method, params, state.as_ref()) {
                let update = SessionNotification::new(session.id.clone(), v1::update(event));
                if cx.send_notification(update).is_err() {
                    return;
                }
            }
        }
    }
}
