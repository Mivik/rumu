//! Other machines' rumu, reached through this one. The web keeps one ws, to
//! this server: a frame naming a `machine` goes on to that machine's ws, and
//! what comes back is tagged with its name (`uplink`). Blobs and plugin files
//! go over HTTP, `/m/<name>/<path>` (`forward`). Either way the machine's
//! token, from `settings.yaml`, replaces this server's sign-in.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use rumu_core::MachineSettings;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{info, warn};
use yawc::{Frame, HttpRequest, MaybeTlsStream, OpCode, Options, WebSocket};

use crate::ws::AppState;

/// Request headers that stay here: this server's sign-in, and what the HTTP
/// client sets itself.
const DROP_REQUEST: &[header::HeaderName] = &[
    header::HOST,
    header::COOKIE,
    header::ORIGIN,
    header::AUTHORIZATION,
    header::CONNECTION,
    header::TRANSFER_ENCODING,
    header::CONTENT_LENGTH,
];

/// Response headers that stay there: its cookie would replace this one's
/// (they share the name).
const DROP_RESPONSE: &[header::HeaderName] = &[
    header::SET_COOKIE,
    header::CONNECTION,
    header::TRANSFER_ENCODING,
];

/// The JSON-RPC error code of a request to a machine that is not connected.
const UNREACHABLE: i64 = -32020;

/// How long connecting to a machine may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest wait between attempts to reach a machine.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

fn unknown(name: &str) -> Response {
    (StatusCode::NOT_FOUND, format!("no machine `{name}`\n")).into_response()
}

/// `/m/<name>/<path>`: an HTTP request (blobs, plugin files) for the machine.
pub async fn forward(
    State(state): State<AppState>,
    Path((name, rest)): Path<(String, String)>,
    request: Request,
) -> Response {
    let Some(m) = state.settings.machine(&name) else {
        return unknown(&name);
    };
    let query = request
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("{}/{rest}{query}", m.url);
    let (parts, body) = request.into_parts();
    let mut headers = parts.headers;
    for h in DROP_REQUEST {
        headers.remove(h);
    }
    let sent = state
        .http
        .request(parts.method, &url)
        .headers(headers)
        .bearer_auth(&m.token)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let res = match sent {
        Ok(res) => res,
        Err(e) => {
            warn!(machine = name, error = %e, "forwarding to machine failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("machine `{name}` unreachable\n"),
            )
                .into_response();
        }
    };
    let mut out = Response::builder().status(res.status());
    let headers = out.headers_mut().expect("fresh builder");
    for (k, v) in res.headers() {
        if !DROP_RESPONSE.contains(k) {
            headers.append(k, v.clone());
        }
    }
    out.body(Body::from_stream(
        res.bytes_stream().map_err(std::io::Error::other),
    ))
    .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

/// The machine a client frame is for, and the frame to send it (its id,
/// and the frame without `machine`, which the machine would forward on):
/// `None` for a frame this server answers itself.
pub fn target(raw: &str) -> Option<(String, Outgoing)> {
    #[derive(Deserialize)]
    struct Route {
        machine: Option<String>,
    }
    let r: Route = serde_json::from_str(raw).ok()?;
    let machine = r.machine.filter(|m| !m.is_empty())?;
    let mut frame: Value = serde_json::from_str(raw).ok()?;
    frame.as_object_mut()?.remove("machine");
    let id = frame["id"].clone();
    Some((machine, (id, frame.to_string())))
}

/// A client frame for an uplink: its request id, and the frame as sent.
pub type Outgoing = (Value, String);

/// One client connection's link to machine `name`: frames sent on the
/// returned channel go to the machine's ws, and the machine's frames come
/// back on `down`, tagged `"machine": name`. While the machine cannot be
/// reached, requests are answered with an error and the link keeps trying,
/// backing off; `machine.status` pushes say when it comes up or goes down.
/// The link ends with the client's connection (the channel's sender).
pub fn uplink(
    state: AppState,
    name: String,
    down: mpsc::Sender<Frame>,
) -> mpsc::UnboundedSender<Outgoing> {
    let (up_tx, mut up_rx) = mpsc::unbounded_channel::<Outgoing>();
    tokio::spawn(async move {
        let tag = format!("{{\"machine\":{},", json!(name));
        let tagged = |payload: &[u8]| -> Option<Vec<u8>> {
            let rest = payload.strip_prefix(b"{")?;
            let mut out = Vec::with_capacity(tag.len() + rest.len());
            out.extend_from_slice(tag.as_bytes());
            out.extend_from_slice(rest);
            Some(out)
        };
        let status = |connected: bool| {
            Frame::text(
                json!({
                    "jsonrpc": "2.0",
                    "machine": name,
                    "method": "machine.status",
                    "params": { "connected": connected },
                })
                .to_string(),
            )
        };
        let mut delay = Duration::from_secs(1);
        // Whether the last attempt failed: only the first failure of a run
        // is logged.
        let mut failing = false;
        loop {
            match connect(&state, &name).await {
                Ok(ws) => {
                    if failing {
                        info!(machine = name, "machine reachable again");
                    }
                    failing = false;
                    delay = Duration::from_secs(1);
                    if down.send(status(true)).await.is_err() {
                        return;
                    }
                    let (mut u_tx, mut u_rx) = ws.split();
                    loop {
                        tokio::select! {
                            out = up_rx.recv() => match out {
                                Some((_, text)) => {
                                    if u_tx.send(Frame::text(text)).await.is_err() {
                                        break;
                                    }
                                }
                                None => return,
                            },
                            frame = u_rx.next() => match frame {
                                Some(f) if f.opcode() == OpCode::Text => {
                                    let Some(out) = tagged(f.payload()) else { continue };
                                    if down.send(Frame::text(out)).await.is_err() {
                                        return;
                                    }
                                }
                                Some(f) if f.opcode() == OpCode::Close => break,
                                Some(_) => {}
                                None => break,
                            },
                        }
                    }
                    warn!(machine = name, "machine connection lost");
                }
                Err(e) => {
                    if !failing {
                        warn!(machine = name, error = e, "connecting to machine failed");
                    }
                    failing = true;
                }
            }
            if down.send(status(false)).await.is_err() {
                return;
            }
            // Until the next attempt, requests fail at once.
            let wake = tokio::time::sleep(delay);
            tokio::pin!(wake);
            delay = (delay * 2).min(MAX_BACKOFF);
            loop {
                tokio::select! {
                    () = &mut wake => break,
                    out = up_rx.recv() => match out {
                        Some((id, _)) if !id.is_null() => {
                            let error = json!({
                                "jsonrpc": "2.0",
                                "id": id,
                                "error": {
                                    "code": UNREACHABLE,
                                    "message": format!("machine `{name}` unreachable"),
                                },
                            });
                            if down.send(Frame::text(error.to_string())).await.is_err() {
                                return;
                            }
                        }
                        Some(_) => {}
                        None => return,
                    },
                }
            }
        }
    });
    up_tx
}

/// Open machine `name`'s ws, with its token.
async fn connect(
    state: &AppState,
    name: &str,
) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let Some(MachineSettings { url, token }) = state.settings.machine(name) else {
        return Err("no such machine".into());
    };
    let url = ws_url(&url).ok_or("bad machine URL")?;
    let options = Options::default()
        .with_limits(crate::ws::WS_MESSAGE_LIMIT, crate::ws::WS_MESSAGE_LIMIT)
        .with_balanced_compression();
    let connecting = WebSocket::connect(url).with_options(options).with_request(
        HttpRequest::builder().header(header::AUTHORIZATION, format!("Bearer {token}")),
    );
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Ok(Ok(ws)) => Ok(ws),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

fn ws_url(base: &str) -> Option<url::Url> {
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/ws")
    } else {
        format!("ws://{}/ws", base.strip_prefix("http://")?)
    };
    ws.parse().ok()
}
