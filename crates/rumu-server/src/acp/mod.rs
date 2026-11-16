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

mod events;
mod session;
mod v1;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, CancelNotification, Implementation, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PromptCapabilities, PromptRequest, PromptResponse,
    SessionNotification, StopReason,
};
use agent_client_protocol::v1::{ConnectionTo, Responder};
use agent_client_protocol::{Agent, Client, Error, ErrorCode, Stdio};
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

use crate::{boot, ws};
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
            .map_err(|(_, message)| message)
    }

    /// Every push from now on.
    fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.state.kernel.subscribe()
    }
}

/// The agent's state across the connection.
struct Acp {
    rumu: Rumu,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
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
    });
    Agent
        .v1()
        .name("rumu")
        .on_receive_request(
            async move |_: InitializeRequest, responder: Responder<InitializeResponse>, _cx| {
                responder.respond(initialize_response())
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
        .prompt_capabilities(PromptCapabilities::new().embedded_context(true));
    InitializeResponse::new(ProtocolVersion::V1)
        .agent_capabilities(capabilities)
        .agent_info(Implementation::new("rumu", env!("CARGO_PKG_VERSION")).title("rumu"))
}

async fn new_session(acp: &Acp, req: &NewSessionRequest) -> Result<NewSessionResponse, Error> {
    let session = Session::create(&acp.rumu, &req.cwd)
        .await
        .map_err(internal)?;
    acp.sessions
        .lock()
        .unwrap()
        .insert(session.id.clone(), session.clone());
    Ok(NewSessionResponse::new(session.id.clone()))
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
        for session in sessions {
            for event in session.feed(method, params) {
                let update = SessionNotification::new(session.id.clone(), v1::update(event));
                if cx.send_notification(update).is_err() {
                    return;
                }
            }
        }
    }
}
