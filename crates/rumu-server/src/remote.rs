//! Other machines' rumu, reached through this one: `/m/<name>/ws` and
//! `/m/<name>/<path>` go on to the machine `settings.yaml` names, with its
//! token in place of this one's. The web shows every machine from one page
//! this way, so this server is the only one it has to sign in to.

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use tracing::warn;
use yawc::{HttpRequest, IncomingUpgrade, OpCode, Options, WebSocket};

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

/// `/m/<name>/ws`: the machine's ws, frame for frame.
pub async fn forward_ws(
    State(state): State<AppState>,
    Path(name): Path<String>,
    ws: IncomingUpgrade,
) -> Response {
    let Some(m) = state.settings.machine(&name) else {
        return unknown(&name);
    };
    let url = match ws_url(&m.url) {
        Some(url) => url,
        None => return (StatusCode::BAD_GATEWAY, "bad machine URL\n").into_response(),
    };
    // Connect before answering the browser, so an unreachable machine is an
    // error it sees (and backs off from) rather than an open, dead socket.
    let options =
        || Options::default().with_limits(crate::ws::WS_MESSAGE_LIMIT, crate::ws::WS_MESSAGE_LIMIT);
    let upstream = WebSocket::connect(url)
        .with_options(options().with_balanced_compression())
        .with_request(
            HttpRequest::builder().header(header::AUTHORIZATION, format!("Bearer {}", m.token)),
        )
        .await;
    let upstream = match upstream {
        Ok(ws) => ws,
        Err(e) => {
            warn!(machine = name, error = %e, "connecting to machine failed");
            return (
                StatusCode::BAD_GATEWAY,
                format!("machine `{name}` unreachable\n"),
            )
                .into_response();
        }
    };
    let Ok((response, upgrade)) = ws.upgrade(options().with_balanced_compression()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    tokio::spawn(async move {
        let Ok(browser) = upgrade.await else { return };
        let (mut b_tx, mut b_rx) = browser.split();
        let (mut u_tx, mut u_rx) = upstream.split();
        let up = async {
            while let Some(f) = b_rx.next().await {
                let close = f.opcode() == OpCode::Close;
                if u_tx.send(f).await.is_err() || close {
                    break;
                }
            }
        };
        let down = async {
            while let Some(f) = u_rx.next().await {
                let close = f.opcode() == OpCode::Close;
                if b_tx.send(f).await.is_err() || close {
                    break;
                }
            }
        };
        // Either side going away ends both.
        tokio::select! {
            () = up => {}
            () = down => {}
        }
        let _ = b_tx.close().await;
        let _ = u_tx.close().await;
    });
    response.into_response()
}

fn ws_url(base: &str) -> Option<url::Url> {
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/ws")
    } else {
        format!("ws://{}/ws", base.strip_prefix("http://")?)
    };
    ws.parse().ok()
}
