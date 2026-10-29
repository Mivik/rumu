//! WebSocket protocol and static serving.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::header::{CONTENT_TYPE, HOST, ORIGIN};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use rumu_core::{BranchId, NodeId, SessionId, TaskId};
use rumu_kernel::Kernel;
use rust_embed::{EmbeddedFile, RustEmbed};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

/// A WebSocket request failure.
#[derive(Debug, thiserror::Error)]
enum ServerError {
    /// A kernel operation failed.
    #[error(transparent)]
    Kernel(#[from] rumu_kernel::Error),

    /// The card library failed.
    #[error(transparent)]
    Cards(#[from] rumu_cards::Error),

    /// A response value could not be serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// The method name is not known.
    #[error("unknown method `{0}`")]
    UnknownMethod(String),

    /// A required string parameter is missing.
    #[error("missing string param `{0}`")]
    MissingParam(String),
}

impl ServerError {
    /// The JSON-RPC 2.0 error code.
    fn code(&self) -> i64 {
        match self {
            Self::UnknownMethod(_) => -32601,
            Self::MissingParam(_) => -32602,
            _ => -32000,
        }
    }
}

type Result<T> = std::result::Result<T, ServerError>;

/// Shared handler state: the kernel and RUMU_HOME (for the home card library).
#[derive(Clone)]
struct AppState {
    kernel: Arc<Kernel>,
    home: PathBuf,
}

/// The built frontend. Debug builds read `web/dist` from disk, release builds
/// embed it; a missing folder is tolerated so `cargo build` works fresh.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct WebAssets;

pub fn router(kernel: Arc<Kernel>, home: PathBuf) -> Router {
    let state = AppState { kernel, home };
    Router::new()
        .route("/ws", get(ws_upgrade))
        .fallback(static_asset)
        .with_state(state)
}

/// Serve an embedded asset, SPA-style: unknown paths get `index.html` with a
/// 200, except missing `_nuxt/` assets, which 404 so a stale bundle fails
/// visibly.
async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = WebAssets::get(path) {
        return serve(file);
    }
    let Some(index) = WebAssets::get("index.html") else {
        return not_found().into_response();
    };
    if path.starts_with("_nuxt/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(index)
}

fn serve(file: EmbeddedFile) -> Response {
    let mime = file.metadata.mimetype().to_owned();
    ([(CONTENT_TYPE, mime)], file.data).into_response()
}

fn not_found() -> &'static str {
    "rumu server is running (no web/dist found; run `pnpm build` in web/ and rebuild; connect via /ws)\n"
}

async fn ws_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            "cross-origin websocket upgrade refused\n",
        )
            .into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

/// Browsers send `Origin` on WebSocket upgrades; refuse cross-site ones so a
/// visited page cannot drive a local agent (cross-site WebSocket hijacking).
/// Same-origin (`Origin`'s host:port equals `Host`) and the Nuxt dev server
/// are allowed; clients without an `Origin` (the smoke example, tests) pass.
/// The host must be `localhost` or an IP literal: a DNS-rebound domain would
/// otherwise be "same-origin" with itself.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Some(authority) = origin
        .to_str()
        .ok()
        .and_then(|o| o.parse::<Uri>().ok())
        .and_then(|u| u.authority().cloned())
    else {
        return false;
    };
    let host = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    let name = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let local_name = name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok();
    let a = authority.as_str();
    (local_name && a.eq_ignore_ascii_case(host))
        || a.eq_ignore_ascii_case("localhost:5173")
        || a.eq_ignore_ascii_case("127.0.0.1:5173")
}

async fn handle_socket(state: AppState, socket: WebSocket) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Message>(64);

    // Single writer task owns the sink: pushes and replies share the channel.
    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(msg).await.is_err() {
                break;
            }
        }
    });

    let mut sub = state.kernel.subscribe();
    loop {
        tokio::select! {
            push = sub.recv() => match push {
                Ok(v) => {
                    if tx.send(Message::Text(v.to_string().into())).await.is_err() {
                        break;
                    }
                }
                // The client missed pushes: close so it reconnects and
                // re-fetches state (web/app/lib/api.ts).
                Err(RecvError::Lagged(n)) => {
                    warn!(dropped = n, "ws client lagged; closing");
                    break;
                }
                Err(RecvError::Closed) => break, // kernel gone
            },
            msg = receiver.next() => match msg {
                // Requests run concurrently so a slow one (task.kill waits up
                // to 5 s) does not block the others; replies share `tx`.
                Some(Ok(Message::Text(text))) => {
                    let state = state.clone();
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let reply = handle_request(&state, text.as_str()).await;
                        let _ = tx.send(Message::Text(reply.into())).await;
                    });
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(_)) => break,
            },
        }
    }
    writer.abort();
}

async fn handle_request(state: &AppState, raw: &str) -> String {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return error_frame(Value::Null, -32700, format!("parse error: {e}")).to_string();
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let params = req.get("params").cloned().unwrap_or(json!({}));
    match dispatch(state, &method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err(e) => error_frame(id, e.code(), e.to_string()).to_string(),
    }
}

/// A JSON-RPC 2.0 error response.
fn error_frame(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

async fn dispatch(state: &AppState, method: &str, p: &Value) -> Result<Value> {
    let kernel = &state.kernel;
    match method {
        "session.list" => Ok(json!(kernel.list_sessions()?)),

        "session.create" => {
            // Optional initial default workspace (a preset name or `@rumu`).
            let workspace = p.get("workspace").and_then(Value::as_str);
            let (session, branch) = kernel.create_session(workspace.map(str::to_owned)).await?;
            Ok(json!({ "session": session, "branch": branch }))
        }

        "session.tree" => {
            let session: SessionId = str_param(p, "session")?.into();
            let (session, nodes, branches) = kernel.session_tree(&session)?;
            let code_calls = kernel.code_calls(&session.id)?;
            Ok(json!({
                "session": session,
                "nodes": nodes,
                "branches": branches,
                "code_calls": code_calls,
            }))
        }

        "branch.path" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let (nodes, state) = kernel.branch_path(&branch)?;
            Ok(json!({ "nodes": nodes, "state": state }))
        }

        "prompt" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let text = str_param(p, "text")?;
            kernel.prompt(&branch, text).await?;
            Ok(json!({}))
        }

        "abort" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.abort(&branch)?;
            Ok(json!({}))
        }

        "branch.fork" => {
            let node: NodeId = str_param(p, "node")?.into();
            let branch = kernel.fork(&node).await?;
            Ok(json!({ "branch": branch }))
        }

        "branch.revert" => {
            let node: NodeId = str_param(p, "node")?.into();
            let lesson = p.get("lesson").and_then(Value::as_str).unwrap_or_default();
            let branch = kernel.revert(&node, lesson).await?;
            Ok(json!({ "branch": branch }))
        }

        "branch.regenerate" => {
            let node: NodeId = str_param(p, "node")?.into();
            let branch = kernel.regenerate(&node).await?;
            Ok(json!({ "branch": branch }))
        }

        "branch.activate" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.activate_branch(&branch)?;
            Ok(json!({}))
        }

        // Runtime-only background tasks (`task.started/progress/done` pushes).
        "task.list" => {
            let session: SessionId = str_param(p, "session")?.into();
            Ok(json!(kernel.tasks(&session)?))
        }

        "task.kill" => {
            let id: TaskId = str_param(p, "id")?.into();
            Ok(json!({ "task": kernel.kill_task(&id).await? }))
        }

        // The home card library (`@rumu:/cards`).
        "card.list" => {
            let store = rumu_cards::FsCardStore::home(&state.home);
            let out = match rumu_cards::library::card_infos(&store) {
                Ok(infos) => infos,
                Err(e) => {
                    warn!(error = %e, "card.list failed");
                    vec![]
                }
            };
            Ok(serde_json::to_value(out)?)
        }

        other => Err(ServerError::UnknownMethod(other.to_owned())),
    }
}

fn str_param<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::MissingParam(key.to_owned()))
}
