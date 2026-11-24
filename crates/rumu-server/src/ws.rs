//! WebSocket protocol, blob routes and static serving.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{SinkExt, StreamExt};
use rumu_blob::Blobs;
use rumu_cards::cards::default_mount_point;
use rumu_cards::{Author, Cards, Decks, library};
use rumu_core::{
    BranchId, BranchKind, CardMount, CardUnmount, Content, NodeId, SessionId, SessionRoot, TaskId,
    Workspace, Workspaces,
};
use rumu_kernel::Kernel;
use rumu_plugins::{Assistant, assistant_of};

use crate::settings::{Settings, SettingsError};
use crate::window::{self, Span};
use rust_embed::{EmbeddedFile, RustEmbed};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;
use tracing::warn;

/// The upload body limit: the largest ingest accepts, plus headroom for
/// encoding overhead. Anything larger is axum's own 413.
const BLOB_BODY_LIMIT: usize = rumu_blob::MAX_INPUT_BYTES + 64 * 1024;

/// A WebSocket request failure.
#[derive(Debug, thiserror::Error)]
enum ServerError {
    /// A kernel operation failed.
    #[error(transparent)]
    Kernel(#[from] rumu_kernel::Error),

    /// A deck or card operation failed.
    #[error(transparent)]
    Cards(#[from] rumu_cards::Error),

    /// A blob-store operation failed (unknown hash, refused upload, …).
    #[error(transparent)]
    Blob(#[from] rumu_blob::Error),

    /// A response value could not be serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// The method name is not known.
    #[error("unknown method `{0}`")]
    UnknownMethod(String),

    /// A required string parameter is missing.
    #[error("missing string param `{0}`")]
    MissingParam(String),

    /// A request parameter has the wrong shape.
    #[error("invalid params: {0}")]
    InvalidParams(String),

    /// A workspace file could not be opened or saved (`fs.snapshot`,
    /// `fs.save_page`).
    #[error("{0}")]
    File(String),

    /// An assistant edit was refused (validation or writing `deck.yaml`).
    #[error(transparent)]
    Plugins(#[from] rumu_plugins::Error),

    /// A settings edit was refused (validation or writing `settings.yaml`).
    #[error(transparent)]
    Settings(#[from] SettingsError),

    /// The request names an assistant that is not configured.
    #[error("unknown assistant `{0}`")]
    UnknownAssistant(String),

    /// An MCP App view's request to its server failed.
    #[error("{0}")]
    Mcp(String),
}

impl ServerError {
    /// The JSON-RPC 2.0 error code.
    fn code(&self) -> i64 {
        match self {
            Self::UnknownMethod(_) => -32601,
            Self::MissingParam(_) | Self::InvalidParams(_) => -32602,
            // A stale write: the web shows its "this card changed" banner.
            Self::Cards(rumu_cards::Error::Stale { .. }) => -32010,
            // The main line cannot move yet: the web asks to stop what holds
            // it (`data.tasks`) and sends the move again with `stop`.
            Self::Kernel(rumu_kernel::Error::LineBusy(_)) => -32011,
            _ => -32000,
        }
    }

    /// The JSON-RPC error's `data`, for the errors the web acts on.
    fn data(&self) -> Option<Value> {
        match self {
            Self::Kernel(rumu_kernel::Error::LineBusy(tasks)) => Some(json!({ "tasks": tasks })),
            _ => None,
        }
    }
}

type Result<T> = std::result::Result<T, ServerError>;

/// Shared handler state: the kernel, the deck registry, the workspace
/// registry (saved under `home`) and the blob store behind image attachments.
#[derive(Clone)]
pub struct AppState {
    pub kernel: Arc<Kernel>,
    pub decks: Decks,
    pub home: std::path::PathBuf,
    pub workspaces: Workspaces,
    pub blobs: Blobs,
    pub settings: Arc<Settings>,
    pub mcp: Arc<rumu_plugins::McpHub>,
}

/// The built frontend. Debug builds read `web/dist` from disk, release builds
/// embed it; a missing folder is tolerated so `cargo build` works fresh.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct WebAssets;

pub fn router(state: AppState, token: crate::auth::Token) -> Router {
    let routes = Router::new()
        .route("/ws", get(ws_upgrade))
        // Registered routes take precedence over the SPA fallback below, so
        // `/blobs` is never swallowed by index.html.
        .route(
            "/blobs",
            post(blob_upload).layer(DefaultBodyLimit::max(BLOB_BODY_LIMIT)),
        )
        .route("/blobs/{hash}", get(blob_get))
        .route("/plugins/{name}/{*file}", get(plugin_file));
    crate::auth::protect(routes, token)
        .fallback(static_asset)
        .with_state(state)
}

/// Serve an embedded asset, SPA-style: unknown paths get `index.html` with a
/// 200, except missing `_nuxt/` assets, which 404 so a stale bundle fails
/// visibly.
///
/// The bundle under `_nuxt/` is content-hashed and cached for good; the rest
/// (`index.html`, `sw.js`, the `_nuxt/builds/` manifests the page polls for
/// a new version) is revalidated every time.
async fn static_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if let Some(file) = WebAssets::get(path) {
        let immutable = path.starts_with("_nuxt/") && !path.starts_with("_nuxt/builds/");
        return serve(file, immutable);
    }
    let Some(index) = WebAssets::get("index.html") else {
        return not_found().into_response();
    };
    if path.starts_with("_nuxt/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    serve(index, false)
}

fn serve(file: EmbeddedFile, immutable: bool) -> Response {
    let mime = file.metadata.mimetype().to_owned();
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [(CONTENT_TYPE, mime), (CACHE_CONTROL, cache.to_owned())],
        file.data,
    )
        .into_response()
}

fn not_found() -> &'static str {
    "rumu server is running (no web/dist found; run `pnpm build` in web/ and rebuild; connect via /ws)\n"
}

/// `GET /plugins/{name}/{file}`: a file beside running plugin `name`'s web
/// entry (the entry itself, its chunks and styles). Its URL carries a
/// version, so it is revalidated rather than cached.
async fn plugin_file(
    State(state): State<AppState>,
    Path((name, file)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Response {
    let found = state
        .settings
        .plugins()
        .and_then(|p| p.web_dir(&name))
        .and_then(|dir| {
            let dir = dir.canonicalize().ok()?;
            let path = dir.join(&file).canonicalize().ok()?;
            path.starts_with(&dir).then_some(path)
        });
    let Some(path) = found else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tower_http::services::ServeFile::new(path)
        .try_call(req)
        .await
    {
        Ok(res) => {
            let mut res = res.map(axum::body::Body::new);
            res.headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            res
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn ws_upgrade(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

/// `POST /blobs?name=<url-encoded file name>`: the raw request body is the
/// file's bytes. An image is normalized once (orientation, size, format); a
/// document is stored as it is. Either way the JSON
/// [`rumu_core::Attachment`] naming the stored blob comes back. A body the
/// ingest refuses is a 400 naming why.
async fn blob_upload(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    bytes: Bytes,
) -> Response {
    let name = params.get("name").cloned().filter(|n| !n.trim().is_empty());
    let blobs = state.blobs.clone();
    // Normalization is CPU-bound: keep it off the async executor.
    let ingested = tokio::task::spawn_blocking(move || blobs.ingest(&bytes, name)).await;
    match ingested {
        Ok(Ok(attachment)) => Json(attachment).into_response(),
        Ok(Err(e)) => blob_error(&e),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("blob ingest failed: {e}") })),
        )
            .into_response(),
    }
}

/// `GET /blobs/{hash}`: the stored bytes, never publicly cached. With a
/// `type` that [`rumu_core::is_media_type`] accepts (an artifact's image,
/// audio, video or PDF) it is served as that type, with range requests so
/// players can seek. Otherwise an image is served as itself; any other blob
/// only ever as sandboxed plain text, so model-written HTML never runs on
/// rumu's origin — the web renders it in a sandboxed frame instead.
async fn blob_get(
    State(state): State<AppState>,
    Path(hash): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    req: axum::extract::Request,
) -> Response {
    if let Some(mime) = params.get("type").filter(|t| rumu_core::is_media_type(t)) {
        return blob_media(&state.blobs, &hash, mime, req).await;
    }
    let bytes = match state.blobs.get(&hash) {
        Ok(bytes) => bytes,
        Err(e) => return blob_error(&e),
    };
    // `describe` re-derives the MIME from the stored bytes; a hash from the
    // wire is trusted for nothing but addressing the blob.
    let mime = if rumu_blob::is_image(&bytes) {
        match state.blobs.describe(&hash, None) {
            Ok(attachment) => attachment.mime,
            Err(e) => return blob_error(&e),
        }
    } else {
        "text/plain; charset=utf-8".to_owned()
    };
    (
        [
            (CONTENT_TYPE, mime),
            (X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()),
            (CONTENT_SECURITY_POLICY, "sandbox".to_owned()),
        ],
        bytes,
    )
        .into_response()
}

/// A blob served as the media type `mime` (one [`rumu_core::is_media_type`]
/// accepts, none of which runs script on rumu's origin). Sandboxed like any
/// blob, except a PDF: browsers refuse to open their viewer in a sandbox, and
/// the viewer keeps a document's own script away from the page.
async fn blob_media(
    blobs: &Blobs,
    hash: &str,
    mime: &str,
    req: axum::extract::Request,
) -> Response {
    let path = match blobs.path(hash) {
        Ok(path) => path,
        Err(e) => return blob_error(&e),
    };
    let mut res = match tower_http::services::ServeFile::new(path)
        .try_call(req)
        .await
    {
        Ok(res) => res.map(axum::body::Body::new),
        // tower-http 0.7 hands a missing file back as an error, not a 404.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return blob_error(&rumu_blob::Error::NotFound(hash.to_owned()));
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("reading blob: {e}") })),
            )
                .into_response();
        }
    };
    let headers = res.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(mime).expect("a known media type"),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    if mime != "application/pdf" {
        headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    }
    res
}

/// The error response of a blob route: `{"error": message}` with the status
/// the failure asks for (bad input 400, unknown hash 404, the rest 500).
fn blob_error(e: &rumu_blob::Error) -> Response {
    use rumu_blob::Error;
    let status = match e {
        Error::Unsupported | Error::TooLarge { .. } | Error::Decode(_) | Error::NotText => {
            StatusCode::BAD_REQUEST
        }
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::InvalidHash(_) => StatusCode::BAD_REQUEST,
        Error::Io(_) | Error::Encode(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({ "error": e.to_string() }))).into_response()
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

    let mut feed = Feed::default();
    let mut sub = state.kernel.subscribe();
    'conn: loop {
        let due = feed.due.unwrap_or_else(Instant::now);
        tokio::select! {
            push = sub.recv() => match push {
                Ok(mut v) => {
                    if let Some(watched) = &feed.session
                        && push_session(&state.kernel, &v).is_some_and(|s| Some(&s) != watched.as_ref())
                    {
                        continue;
                    }
                    if v["method"] == "node.appended" {
                        window::slim(&mut v["params"]["node"]);
                    }
                    for v in feed.take(v) {
                        if tx.send(Message::Text(v.to_string().into())).await.is_err() {
                            break 'conn;
                        }
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
            () = tokio::time::sleep_until(due), if feed.due.is_some() => {
                for v in feed.flush() {
                    if tx.send(Message::Text(v.to_string().into())).await.is_err() {
                        break 'conn;
                    }
                }
            },
            msg = receiver.next() => match msg {
                // Requests run concurrently so a slow one (task.kill waits up
                // to 5 s) does not block the others; replies share `tx`.
                Some(Ok(Message::Text(text))) => {
                    // Answered in order, not spawned: the pushes after the
                    // reply, and the requests after it, see the new filter.
                    if let Some(reply) = feed.control(&state.kernel, text.as_str()) {
                        if tx.send(Message::Text(reply.to_string().into())).await.is_err() {
                            break;
                        }
                        continue;
                    }
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

/// How long a client's live output (`tool.progress` / `task.progress`
/// appends) gathers before it goes out as one push: a command printing in
/// 4 KiB reads would otherwise send hundreds a second.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// How long a model call's `llm.delta` pushes gather into one: a stream
/// sends a few dozen a second, each mostly envelope. The web paints
/// streaming thinking at this pace anyway.
const DELTA_EVERY: Duration = Duration::from_millis(50);

/// The most of one gathered append that goes out (its tail, in bytes).
/// The web shows only the last 64K chars of live output, which this always
/// covers, so a flood drops nothing it would show.
const PROGRESS_TAIL: usize = 256 * 1024;

/// How much of a prompt's text `branch.prompts` sends (chars): a one-line
/// label shows less, and anchors sit near the start.
const PROMPT_PREVIEW: usize = 200;

/// One client's push filters, and its live output gathering.
#[derive(Default)]
struct Feed {
    /// `session.watch`: the session this client shows (`Some(None)`: none).
    /// Until it is called, the client gets every push.
    session: Option<Option<SessionId>>,
    /// `task.watch`: the tasks whose output appends this client takes, each
    /// with where the buffer it was handed ended. Until it is called, every
    /// task's.
    tasks: Option<HashMap<String, u64>>,
    /// Appends and model deltas waiting to go out, one per tool call, task,
    /// or run of one branch's same-kind deltas.
    pending: Vec<Value>,
    /// When `pending` goes out.
    due: Option<Instant>,
}

impl Feed {
    /// Answer a request the connection itself handles (`session.watch`,
    /// `task.watch`): in order, so the pushes after the reply see the new
    /// filter.
    fn control(&mut self, kernel: &Kernel, raw: &str) -> Option<Value> {
        #[derive(Deserialize)]
        struct Req {
            #[serde(default)]
            id: Value,
            method: String,
            #[serde(default)]
            params: Params,
        }
        #[derive(Deserialize, Default)]
        struct Params {
            session: Option<SessionId>,
            #[serde(default)]
            ids: Vec<String>,
        }
        let req: Req = serde_json::from_str(raw).ok()?;
        let result = match req.method.as_str() {
            // `{ session: id | null }`: take only the pushes about that
            // session (and the ones every client gets).
            "session.watch" => {
                self.session = Some(req.params.session);
                Value::Null
            }
            // `{ ids }`: take output appends of these tasks only. Each one
            // newly watched comes with the tail of its buffered output;
            // appends that buffer already held are not sent again.
            "task.watch" => {
                let mut old = self.tasks.take().unwrap_or_default();
                let mut tasks = HashMap::new();
                let mut output = serde_json::Map::new();
                for id in req.params.ids {
                    if let Some(end) = old.remove(&id) {
                        tasks.insert(id, end);
                    } else if let Some((mut text, end)) = kernel.task_buffer(&TaskId(id.clone())) {
                        keep_tail(&mut text, PROGRESS_TAIL);
                        output.insert(id.clone(), json!(text));
                        tasks.insert(id, end);
                    }
                }
                self.tasks = Some(tasks);
                json!({ "output": output })
            }
            _ => return None,
        };
        Some(json!({ "jsonrpc": "2.0", "id": req.id, "result": result }))
    }

    /// Take one kernel push the session filter let through: what to send
    /// now. An output append waits to go out with the next ones; anything
    /// else sends the waiting appends first, so they keep their order
    /// around it.
    fn take(&mut self, mut push: Value) -> Vec<Value> {
        if push["method"] == "task.progress"
            && let Some(tasks) = &self.tasks
            && let Some(params) = push["params"].as_object_mut()
            && params.contains_key("append")
        {
            let held = params["id"].as_str().and_then(|id| tasks.get(id));
            let end = params.get("end").and_then(Value::as_u64).unwrap_or(0);
            // Unwatched, or already in the buffer `task.watch` handed over.
            if held.is_none_or(|&held| end <= held) {
                params.remove("append");
                params.remove("end");
                if !params.contains_key("data") && !params.contains_key("kind") {
                    return Vec::new();
                }
            }
        }
        if let Some(key) = append_key(&push) {
            self.gather(key, push);
            return Vec::new();
        }
        if push["method"] == "llm.delta" {
            self.gather_delta(push);
            return Vec::new();
        }
        let mut out = self.flush();
        out.push(push);
        out
    }

    /// Hold an item until `every` from now, or sooner if something already
    /// waits.
    fn hold(&mut self, push: Value, every: Duration) {
        self.pending.push(push);
        let at = Instant::now() + every;
        self.due = Some(self.due.map_or(at, |d| d.min(at)));
    }

    /// Add a model call's delta to the one waiting for the same branch,
    /// when that is of the same kind (text / reasoning): one push carries
    /// the run of deltas, with the last one's `seq`, the first one's `at`
    /// and `from` (its `seq`). A kind switch starts a new push, so the
    /// client sees reasoning and text in the order they came.
    fn gather_delta(&mut self, push: Value) {
        let kind = |v: &Value| {
            ["text", "reasoning"]
                .into_iter()
                .find(|k| v["params"][k].is_string())
        };
        let branch = &push["params"]["branch"];
        let waiting = self
            .pending
            .iter_mut()
            .rev()
            .find(|v| v["method"] == "llm.delta" && &v["params"]["branch"] == branch);
        let (Some(waiting), Some(k)) = (waiting, kind(&push)) else {
            return self.hold(push, DELTA_EVERY);
        };
        if kind(waiting) != Some(k) {
            return self.hold(push, DELTA_EVERY);
        }
        let params = &mut waiting["params"];
        if params.get("from").is_none() {
            params["from"] = params["seq"].clone();
        }
        let mut text = params[k].as_str().unwrap_or_default().to_owned();
        text.push_str(push["params"][k].as_str().unwrap_or_default());
        params[k] = json!(text);
        params["seq"] = push["params"]["seq"].clone();
    }

    /// Add an append to the one waiting for the same call or task.
    fn gather(&mut self, key: (String, String), push: Value) {
        let waiting = self
            .pending
            .iter_mut()
            .find(|v| append_key(v).as_ref() == Some(&key));
        let Some(waiting) = waiting else {
            return self.hold(push, PROGRESS_EVERY);
        };
        let params = &mut waiting["params"];
        let mut text = params["append"].as_str().unwrap_or_default().to_owned();
        text.push_str(push["params"]["append"].as_str().unwrap_or_default());
        keep_tail(&mut text, PROGRESS_TAIL);
        params["append"] = json!(text);
        if let Some(end) = push["params"].get("end") {
            params["end"] = end.clone();
        }
    }

    /// The waiting appends, now due.
    fn flush(&mut self) -> Vec<Value> {
        self.due = None;
        std::mem::take(&mut self.pending)
    }
}

/// Drop the front of `text` down to its last `limit` bytes, on a char
/// boundary.
fn keep_tail(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut cut = text.len() - limit;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text.drain(..cut);
    }
}

/// What an output-only progress push is about (method, then the call or
/// task), or `None` for any other push.
fn append_key(push: &Value) -> Option<(String, String)> {
    let method = push["method"].as_str()?;
    let p = push["params"].as_object()?;
    if !p.contains_key("append") || p.contains_key("data") || p.contains_key("kind") {
        return None;
    }
    let key = match method {
        "tool.progress" => format!(
            "{}:{}",
            p.get("branch")?.as_str()?,
            p.get("call_id")?.as_str()?
        ),
        "task.progress" => p.get("id")?.as_str()?.to_owned(),
        _ => return None,
    };
    Some((method.to_owned(), key))
}

/// The session a push is about: the one owning the branch it names. `None`
/// for pushes every client gets (lists, settings, pending decisions — the
/// sidebar shows every session's waiting ask).
fn push_session(kernel: &Kernel, push: &Value) -> Option<SessionId> {
    if push["method"].as_str()?.starts_with("decision.") {
        return None;
    }
    let p = &push["params"];
    match &p["branch"] {
        Value::Object(b) => b.get("session")?.as_str().map(SessionId::from),
        Value::String(b) => kernel.session_of(&BranchId::from(b.as_str())).ok(),
        _ => {
            let b = p["task"]["branch"].as_str()?;
            kernel.session_of(&BranchId::from(b)).ok()
        }
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(p: &Value) -> Result<T> {
    serde_json::from_value(p.clone()).map_err(|e| ServerError::InvalidParams(e.to_string()))
}

/// Run a settings edit, and push the result to every client.
fn settings_edit(
    state: &AppState,
    f: impl FnOnce(&Settings) -> std::result::Result<rumu_core::SettingsInfo, SettingsError>,
) -> Result<Value> {
    let info = json!(f(&state.settings)?);
    state.kernel.settings_changed(info.clone());
    Ok(info)
}

async fn handle_request(state: &AppState, raw: &str) -> String {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            return error_frame(Value::Null, -32700, format!("parse error: {e}"), None).to_string();
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let params = req.get("params").cloned().unwrap_or(json!({}));
    match call(state, &method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err((code, message, data)) => error_frame(id, code, message, data).to_string(),
    }
}

/// Run one request as a WebSocket client would: its result, or the JSON-RPC
/// error code and message.
pub(crate) async fn call(
    state: &AppState,
    method: &str,
    params: &Value,
) -> std::result::Result<Value, (i64, String, Option<Value>)> {
    dispatch(state, method, params)
        .await
        .map_err(|e| (e.code(), e.to_string(), e.data()))
}

/// The `stop` flag of the methods that move the main line: stop what the
/// move leaves behind instead of refusing.
fn stop_param(p: &Value) -> bool {
    p.get("stop").and_then(Value::as_bool) == Some(true)
}

/// The assistant deck named by the `assistant` id param (the default
/// assistant when omitted).
fn assistant_param(state: &AppState, p: &Value) -> Result<rumu_cards::Deck> {
    let id = p
        .get("assistant")
        .and_then(Value::as_str)
        .map_or_else(|| state.decks.default_assistant(), str::to_owned);
    state
        .decks
        .get(&id)
        .filter(|d| d.assistant())
        .ok_or(ServerError::UnknownAssistant(id))
}

/// A JSON-RPC 2.0 error response.
fn error_frame(id: Value, code: i64, message: String, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

async fn dispatch(state: &AppState, method: &str, p: &Value) -> Result<Value> {
    let kernel = &state.kernel;
    match method {
        // Liveness probe: the web sends it when the page comes back to the
        // foreground, to catch a socket that died while it was suspended.
        "ping" => Ok(json!({})),

        "session.list" => Ok(json!(kernel.list_sessions()?)),

        "session.create" => {
            // The assistant deck id (the default assistant when omitted),
            // with its default mounts, in the `project` deck (when omitted
            // or null, the first project deck among the default mounts).
            let deck = assistant_param(state, p)?;
            let project = project_param(state, p)?;
            let mounts =
                state
                    .decks
                    .session_mounts(deck.id(), project.as_ref().map(|d| d.id()), None);
            let (session, branch) = kernel
                .create_rooted_session(SessionRoot {
                    assistant: deck.id().to_owned(),
                    mounts,
                })
                .await?;
            Ok(json!({ "session": session, "branch": branch }))
        }

        // Move a session into the `project` deck (out of the one it is in),
        // or out of its project (null): recorded on its main line, the model
        // told in a note.
        "session.move" => {
            let session: SessionId = str_param(p, "session")?.into();
            let project = project_param(state, p)?;
            let info = kernel
                .list_sessions()?
                .into_iter()
                .find(|s| s.id == session)
                .ok_or_else(|| ServerError::InvalidParams(format!("no session `{session}`")))?;
            let mounts = kernel.branch_path(&info.main_branch)?.1.mounts;
            let old = mounts.iter().find(|m| m.project);
            if old.map(|m| &m.deck) == project.as_ref().map(|d| &d.meta.id) {
                return Ok(json!({}));
            }
            let mut notes = Vec::new();
            if let Some(old) = old {
                let name = state.decks.get(&old.deck).map(|d| d.meta.name);
                let text = format!(
                    "The user moved this session out of its project `{}`: `@rumu:{}` is \
                     unmounted, and content loaded from it earlier is no longer readable, \
                     writable or updated.",
                    name.as_deref().unwrap_or(&old.deck),
                    old.at
                );
                notes.push(Content::note_with(
                    text,
                    &CardUnmount { at: old.at.clone() },
                ));
            }
            if let Some(deck) = project {
                // A deck mounted already is marked where it is.
                let staying: Vec<CardMount> =
                    mounts.iter().filter(|m| !m.project).cloned().collect();
                let at = staying
                    .iter()
                    .find(|m| m.deck == deck.meta.id)
                    .map(|m| m.at.clone())
                    .unwrap_or_else(|| default_mount_point(&deck.meta.name, "project", &staying));
                let workspaces = state.workspaces.project(deck.id());
                let listed = if workspaces.is_empty() {
                    "It mounts no workspace.".to_owned()
                } else {
                    format!(
                        "Its workspaces: {}.",
                        workspaces
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let text = format!(
                    "The user moved this session into the project `{}` (deck id {}), mounted \
                     at `@rumu:{at}`. {listed}",
                    deck.meta.name,
                    deck.id()
                );
                notes.push(Content::note_with(
                    text,
                    &CardMount::new(deck.id(), at, true),
                ));
            }
            kernel.move_project(&session, notes).await?;
            Ok(json!({}))
        }

        // The project decks, each with the workspaces it mounts. Changes to
        // the lists are pushed as `workspaces.updated`.
        "project.list" => Ok(json!(rumu_plugins::projects(
            &state.decks,
            &state.workspaces
        ))),

        // A new project deck (blank charter) mounting `workspaces`.
        "project.create" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Create {
                name: String,
                #[serde(default)]
                workspaces: Vec<String>,
                #[serde(default)]
                icon: Option<rumu_cards::DeckIcon>,
                #[serde(default)]
                private: bool,
            }
            let req: Create = parse_params(p)?;
            let deck = rumu_plugins::create_project(
                &state.home,
                &state.decks,
                &state.workspaces,
                &req.name,
                req.workspaces,
                req.icon,
                req.private,
            )?;
            Ok(json!({ "project": rumu_plugins::project_of(&deck, &state.workspaces) }))
        }

        // Rename a project, set its icon (an empty one clears it), and/or
        // replace the workspaces it mounts (in order; the first is its
        // default).
        "project.update" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Update {
                id: String,
                #[serde(default)]
                name: Option<String>,
                #[serde(default)]
                workspaces: Option<Vec<String>>,
                #[serde(default)]
                icon: Option<rumu_cards::DeckIcon>,
                #[serde(default)]
                private: Option<bool>,
            }
            let req: Update = parse_params(p)?;
            let deck = project_deck(state, &req.id)?;
            let deck = rumu_plugins::update_project(
                &state.home,
                &state.decks,
                &state.workspaces,
                &deck,
                rumu_plugins::ProjectChange {
                    name: req.name,
                    workspaces: req.workspaces,
                    icon: req.icon,
                    private: req.private,
                },
            )?;
            Ok(json!({ "project": rumu_plugins::project_of(&deck, &state.workspaces) }))
        }

        "assistant.list" => {
            let list: Vec<Assistant> = state
                .decks
                .assistants()
                .iter()
                .map(|d| assistant_of(&state.decks, d))
                .collect();
            Ok(json!(list))
        }

        // Choices for the assistant config form: decks (as mount targets).
        // Workspaces come from `workspace.list`.
        "assistant.options" => {
            let decks: Vec<Value> = state
                .decks
                .list()
                .iter()
                .map(|d| json!({ "id": d.id(), "name": d.meta.name, "assistant": d.assistant() }))
                .collect();
            Ok(json!({ "decks": decks }))
        }

        // Create or replace an assistant deck: its `deck.yaml` and, when given,
        // a pinned `instructions.md`.
        "assistant.upsert" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct MountReq {
                deck: String,
                at: String,
                #[serde(default)]
                write: bool,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Upsert {
                #[serde(default)]
                id: Option<String>,
                name: String,
                #[serde(default)]
                mounts: Vec<MountReq>,
                #[serde(default)]
                instructions: Option<String>,
                #[serde(default)]
                allow_ask: Vec<String>,
                #[serde(default)]
                mcp: rumu_core::McpUse,
                #[serde(default)]
                skills: Vec<String>,
                #[serde(default)]
                cat: Option<rumu_cards::DeckCat>,
                #[serde(default)]
                model: Option<rumu_core::ModelUse>,
            }
            let req: Upsert = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            // No id creates a deck (refused if its directory exists); an id
            // updates exactly that deck.
            let existing = match req.id.as_deref().filter(|id| !id.is_empty()) {
                Some(id) => Some(
                    state
                        .decks
                        .get(id)
                        .ok_or_else(|| ServerError::UnknownAssistant(id.to_owned()))?,
                ),
                None => None,
            };
            let mounts = req
                .mounts
                .into_iter()
                .map(|m| rumu_cards::DeckMount {
                    deck: m.deck,
                    at: m.at,
                    write: m.write,
                })
                .collect();
            // The web edits the whole form: what it leaves out is cleared.
            let deck = rumu_plugins::save_assistant(
                &state.decks,
                existing.as_ref(),
                rumu_plugins::AssistantConfig {
                    name: req.name.clone(),
                    mounts,
                    allow_ask: req.allow_ask,
                    mcp: req.mcp,
                    skills: req.skills,
                    cat: req.cat,
                    model: req.model,
                },
            )?;
            let cards = Cards::library(&state.decks);
            let public = format!("{}/instructions", deck.id());
            match req.instructions.filter(|i| !i.trim().is_empty()) {
                Some(instructions) => {
                    let view = format!(
                        "---\npinned: true\ndescription: Pinned instructions for {}\n---\n{}\n",
                        req.name,
                        instructions.trim_end()
                    );
                    cards.write(&public, &view, None, None, &Author::user())?;
                }
                None if cards.raw(&public)?.is_some() => {
                    cards.remove(&public, None, &Author::user())?;
                }
                None => {}
            }
            Ok(json!({ "deck": assistant_of(&state.decks, &deck) }))
        }

        "assistant.delete" => {
            let id = str_param(p, "id")?;
            state.decks.remove_assistant(id)?;
            Ok(json!({}))
        }

        "assistant.set_default" => {
            let id = str_param(p, "id")?;
            state.decks.set_default_assistant(id)?;
            Ok(json!({}))
        }

        // `settings.yaml`. The API key is write-only.
        "settings.get" => Ok(json!(state.settings.info())),

        // Every edit replies with, and pushes, the whole settings. The web
        // lists a saved provider's models after (`settings.refresh_models`).
        "settings.set_provider" => {
            let edit: crate::settings::ProviderEdit = parse_params(p)?;
            settings_edit(state, |s| s.set_provider(edit))
        }
        "settings.refresh_models" => {
            let id = str_param(p, "id")?;
            state.settings.refresh_models(id).await?;
            settings_edit(state, |s| Ok(s.info()))
        }
        // The endpoints an address serves, for a provider being set up.
        "settings.probe" => {
            let url = str_param(p, "url")?;
            let key = p.get("api_key").and_then(Value::as_str).map(str::to_owned);
            let id = p.get("id").and_then(Value::as_str);
            Ok(json!(state.settings.probe(url, key, id).await))
        }
        "settings.model" => {
            let model = str_param(p, "model")?;
            Ok(json!(state.settings.model(model)?))
        }
        "settings.pin" => {
            let model = str_param(p, "model")?;
            let pinned = p.get("pinned").and_then(Value::as_bool).unwrap_or(true);
            settings_edit(state, |s| s.pin(model, pinned))
        }
        "settings.remove_provider" => {
            let id = str_param(p, "id")?;
            settings_edit(state, |s| s.remove_provider(id))
        }
        "settings.set_alias" => {
            let edit = parse_params(p)?;
            settings_edit(state, |s| s.set_alias(edit))
        }
        "settings.remove_alias" => {
            let name = str_param(p, "name")?;
            settings_edit(state, |s| s.remove_alias(name))
        }
        "settings.set_titles" => {
            let titles = parse_params(&p["titles"])?;
            settings_edit(state, |s| s.set_titles(titles))
        }
        "settings.set_roles" => {
            let roles = parse_params(p)?;
            settings_edit(state, |s| s.set_roles(roles))
        }
        "settings.set_appearance" => {
            let appearance = parse_params(&p["appearance"])?;
            settings_edit(state, |s| s.set_appearance(appearance))
        }
        // MCP servers: their status and tools follow as pushes.
        "settings.set_mcp" => {
            let edit = parse_params(p)?;
            settings_edit(state, |s| s.set_mcp(edit))
        }
        "settings.remove_mcp" => {
            let name = str_param(p, "name")?;
            settings_edit(state, |s| s.remove_mcp(name))
        }
        "settings.install_skills" => {
            let url = str_param(p, "url")?;
            let name = p.get("name").and_then(Value::as_str);
            let info = json!(state.settings.install_skills(url, name).await?);
            state.kernel.settings_changed(info.clone());
            Ok(info)
        }
        "settings.update_skills" => {
            let info = json!(state.settings.update_skills(str_param(p, "name")?).await?);
            state.kernel.settings_changed(info.clone());
            Ok(info)
        }
        "settings.remove_skills" => {
            let name = str_param(p, "name")?;
            settings_edit(state, |s| s.remove_skills(name))
        }
        "settings.reconnect_mcp" => {
            state.settings.reconnect_mcp(str_param(p, "name")?);
            Ok(json!({}))
        }
        // External plugins (`plugins:`): their status follows as pushes.
        "settings.set_plugin" => {
            let edit = parse_params(p)?;
            settings_edit(state, |s| s.set_plugin(edit))
        }
        "settings.remove_plugin" => {
            let name = str_param(p, "name")?;
            settings_edit(state, |s| s.remove_plugin(name))
        }
        // A plugin's web side asking its process (`ctx.call`), on behalf of
        // the session the page shows (`branch`).
        "plugin.call" => {
            let plugin = str_param(p, "plugin")?;
            let method = str_param(p, "method")?;
            let branch = p["branch"].as_str().map(BranchId::from);
            let plugins = state
                .settings
                .plugins()
                .ok_or_else(|| ServerError::InvalidParams(format!("no plugin `{plugin}`")))?;
            plugins
                .call(plugin, method, p["params"].clone(), branch.as_ref())
                .await
                .map_err(|e| ServerError::Plugins(e.into()))
        }
        "settings.restart_plugin" => {
            let name = str_param(p, "name")?;
            if !state.settings.restart_plugin(name) {
                return Err(ServerError::InvalidParams(format!("no plugin `{name}`")));
            }
            Ok(json!({}))
        }

        // The workspace registry (`workspaces.yaml`). Every change is pushed
        // as `workspaces.updated`.
        "workspace.list" => Ok(json!(state.workspaces.list())),

        // Add a workspace, or replace the one of the same name.
        "workspace.upsert" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Upsert {
                name: String,
                root: String,
                #[serde(default)]
                ssh: Option<String>,
                #[serde(default)]
                read_only: bool,
            }
            let req: Upsert = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let ws = Workspace::new(req.name, req.ssh, req.root, req.read_only);
            let ws = rumu_plugins::upsert_workspace(&state.home, &state.workspaces, ws)?;
            Ok(json!({ "workspace": ws }))
        }

        // Remove a workspace, and from every project that mounts it.
        "workspace.delete" => {
            let name = str_param(p, "name")?;
            rumu_plugins::remove_workspace(&state.home, &state.workspaces, name)?;
            Ok(json!({}))
        }

        // The subdirectories of a local absolute path (`~` and the default
        // are $HOME), for picking a workspace root. Dot directories are
        // included; the picker hides them until asked.
        "fs.dirs" => {
            let home = std::env::home_dir().unwrap_or_else(|| "/".into());
            let raw = p.get("path").and_then(Value::as_str).unwrap_or("~");
            let path = match raw.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
                    format!("{}{rest}", home.display())
                }
                _ => raw.to_owned(),
            };
            // Drops trailing slashes and `.` segments.
            let path: std::path::PathBuf = std::path::Path::new(&path).components().collect();
            if !path.is_absolute() {
                return Err(ServerError::InvalidParams(format!(
                    "`{}` is not an absolute path",
                    path.display()
                )));
            }
            let entries = std::fs::read_dir(&path).map_err(|e| {
                ServerError::InvalidParams(format!("cannot list `{}`: {e}", path.display()))
            })?;
            let mut dirs: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()))
                .filter_map(|e| e.file_name().into_string().ok())
                .collect();
            dirs.sort_by_key(|n| n.to_lowercase());
            Ok(json!({ "path": path, "parent": path.parent(), "dirs": dirs }))
        }

        // A workspace file as the branch's tools address it, snapshotted the
        // way `show` would without recording anything: links the web opens.
        "fs.snapshot" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let path = str_param(p, "path")?;
            let (_, path_state) = kernel.branch_path(&branch)?;
            let artifact = rumu_plugins::snapshot_file(&path_state, path, state.blobs.clone())
                .await
                .map_err(ServerError::File)?;
            Ok(json!({ "artifact": artifact }))
        }

        // A reply's widget the user saved: a page in the branch's `@scratch`,
        // snapshotted like `fs.snapshot` and recorded on the branch, so the
        // model learns of the file and the branch lists it as an artifact.
        "fs.save_page" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let page = str_param(p, "page")?;
            let (_, path_state) = kernel.branch_path(&branch)?;
            let artifact = rumu_plugins::save_page(&path_state, page, state.blobs.clone())
                .await
                .map_err(ServerError::File)?;
            let text = format!(
                "The user saved a widget of your reply as {}: a page of its own now, \
which you can read, edit and `show`.",
                artifact.path
            );
            kernel.record_artifact(&branch, text, &artifact).await?;
            Ok(json!({ "artifact": artifact }))
        }

        // MCP Apps: a view of a tool result calls a tool of the same server,
        // reads one of its resources, or tells the branch's model something
        // (`ui/update-model-context`), kept for its next call.
        "mcp.app_call" | "mcp.app_read" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let (_, path_state) = kernel.branch_path(&branch)?;
            let servers = rumu_plugins::mcp::servers(&state.decks, &path_state);
            let server = str_param(p, "server")?;
            let result = if method == "mcp.app_call" {
                let args = match p.get("arguments") {
                    None | Some(Value::Null) => Default::default(),
                    Some(Value::Object(args)) => args.clone(),
                    Some(_) => {
                        return Err(ServerError::InvalidParams(
                            "`arguments` must be an object".into(),
                        ));
                    }
                };
                let tool = str_param(p, "tool")?;
                rumu_plugins::mcp::app_call(&state.mcp, &servers, server, tool, args).await
            } else {
                let uri = str_param(p, "uri")?;
                rumu_plugins::mcp::app_read(&state.mcp, &servers, server, uri).await
            };
            result.map_err(ServerError::Mcp)
        }

        "mcp.app_context" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.branch_path(&branch)?;
            let at = (str_param(p, "server")?, str_param(p, "tool")?);
            let content = p.get("content").unwrap_or(&Value::Null);
            let view = str_param(p, "view")?;
            rumu_plugins::mcp::app_context(
                &state.mcp,
                &branch,
                view,
                at,
                content,
                p.get("structured"),
            );
            Ok(json!({}))
        }

        // Questions a tool call put to the user (`decision.requested`).
        "decision.list" => Ok(json!(kernel.decisions())),

        "decision.answer" => {
            let id = str_param(p, "id")?;
            let answer = p.get("answer").cloned().unwrap_or(Value::Null);
            kernel.answer_decision(id, answer)?;
            Ok(json!({}))
        }

        // The user started on a decision: stop its clock.
        "decision.hold" => {
            kernel.hold_decision(str_param(p, "id")?)?;
            Ok(json!({}))
        }

        // The main line's end (`tail`: its last nodes, from a user message
        // on; all of it without), with the variants and side questions that
        // hang off it. With `from` (a node on it: where a reader left off),
        // `tail` nodes from there on instead, and `end`: their last node,
        // when they stop short of the head; `turns_before` / `turns` as for
        // `branch.window`. `branch.window` fetches what
        // comes before or after, and `branch.nodes` a sub-agent's own. `usage` totals every
        // model call of the session either way, and `agent_usage` each
        // sub-agent's own.
        "session.tree" => {
            let session: SessionId = str_param(p, "session")?.into();
            let tail = p.get("tail").and_then(Value::as_u64).map(|n| n as usize);
            let from: Option<NodeId> = p.get("from").and_then(Value::as_str).map(Into::into);
            let (session, line, branches, usage, agent_usage) =
                kernel.with_tree(&session, |info, nodes, branches| {
                    let line = branches
                        .get(&info.main_branch)
                        .and_then(|b| {
                            let place = from.as_ref().and_then(|id| {
                                let span = Span::From(id, tail.unwrap_or(usize::MAX));
                                window::window(nodes, branches, &b.head, span)
                            });
                            place.or_else(|| {
                                window::window(nodes, branches, &b.head, Span::Tail(tail))
                            })
                        })
                        .map(window::Window::owned)
                        .unwrap_or_default();
                    let mut list: Vec<rumu_core::Branch> = branches.values().cloned().collect();
                    list.sort_by(|a, b| {
                        a.created_at
                            .cmp(&b.created_at)
                            .then_with(|| a.id.cmp(&b.id))
                    });
                    let usage = session_usage(nodes.values());
                    let agent_usage = agent_usage(nodes, branches);
                    (info.clone(), line, list, usage, agent_usage)
                })?;
            let live = kernel.live_turns(&session.id)?;
            Ok(json!({
                "session": session,
                "nodes": line.wire_nodes(),
                "variant_ends": line.ends,
                "end": line.end,
                "first": line.first,
                "turns_before": line.turns_before,
                "turns": line.turns,
                "branches": branches,
                "live": live,
                "usage": usage,
                "agent_usage": agent_usage,
            }))
        }

        // A stretch of `branch`'s path the way `session.tree` gives one:
        // whole turns, with what hangs off them. What came before node
        // `before` (its last `limit` nodes, all without), what comes after
        // node `after` (`limit` nodes, on to the head without, and `end`
        // when it stops short), with `turn` `limit` nodes from that user
        // message on (1-based), with `start` its first `limit` nodes (the
        // reader jumped there), or none of these: the end itself — the head
        // moved somewhere the client has not loaded. `first` is the
        // stretch's first node, `turns_before` / `turns` count the user
        // messages before it and on the whole path.
        "branch.window" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let before: Option<NodeId> = p.get("before").and_then(Value::as_str).map(Into::into);
            let after: Option<NodeId> = p.get("after").and_then(Value::as_str).map(Into::into);
            let limit = p.get("limit").and_then(Value::as_u64).map(|n| n as usize);
            let start = p.get("start").and_then(Value::as_bool).unwrap_or(false);
            let turn = p.get("turn").and_then(Value::as_u64).map(|n| n as usize);
            let span = match (&before, &after, turn) {
                (Some(id), _, _) => Span::Before(id, limit),
                (None, Some(id), _) => Span::After(id, limit.unwrap_or(usize::MAX)),
                (None, None, Some(n)) => Span::Turn(n, limit.unwrap_or(usize::MAX)),
                (None, None, None) if start => Span::Start(limit.unwrap_or(usize::MAX)),
                (None, None, None) => Span::Tail(limit),
            };
            let session = kernel.session_of(&branch)?;
            let line = kernel
                .with_tree(&session, |_, nodes, branches| {
                    let head = &branches.get(&branch)?.head;
                    window::window(nodes, branches, head, span).map(window::Window::owned)
                })?
                .ok_or_else(|| {
                    ServerError::InvalidParams("that node is not on the branch's path".into())
                })?;
            Ok(json!({
                "nodes": line.wire_nodes(),
                "variant_ends": line.ends,
                "end": line.end,
                "first": line.first,
                "turns_before": line.turns_before,
                "turns": line.turns,
            }))
        }

        // The prompts on `branch`'s path from turn `from` on (1-based),
        // `limit` of them (50 without), each a preview — its id, turn number,
        // opening text and attachments — for listing turns not loaded; with
        // `turns`, how many the path has.
        "branch.prompts" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let from = p.get("from").and_then(Value::as_u64).unwrap_or(1) as usize;
            let limit = p
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .min(500) as usize;
            let session = kernel.session_of(&branch)?;
            let (prompts, turns) = kernel
                .with_tree(&session, |_, nodes, branches| {
                    let head = &branches.get(&branch)?.head;
                    let (got, turns) = window::prompts(nodes, head, from, limit);
                    let got: Vec<Value> = got
                        .into_iter()
                        .map(|(turn, n)| {
                            json!({ "id": n.id, "turn": turn, "content": window::prompt_preview(n, PROMPT_PREVIEW) })
                        })
                        .collect();
                    Some((got, turns))
                })?
                .ok_or_else(|| ServerError::InvalidParams("no such branch".into()))?;
            Ok(json!({ "prompts": prompts, "turns": turns }))
        }

        // The nodes of `branch`'s path that `session.tree` leaves out,
        // root → head (none for a main line or a side question).
        "branch.nodes" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let session = kernel.session_of(&branch)?;
            let own = kernel.with_tree(&session, |_, nodes, branches| {
                let agents = window::agent_nodes(nodes, branches);
                let mut own = Vec::new();
                let mut cur = branches.get(&branch).and_then(|b| nodes.get(&b.head));
                while let Some(n) = cur.filter(|n| agents.contains(&n.id)) {
                    own.push(n.clone());
                    cur = n.parent.as_ref().and_then(|p| nodes.get(p));
                }
                own.reverse();
                // and the side questions asked off it, whole
                for b in branches.values() {
                    if !matches!(&b.kind, BranchKind::Side { parent_branch, .. } if parent_branch == &branch) {
                        continue;
                    }
                    let mut cur = nodes.get(&b.head);
                    while let Some(n) = cur.filter(|n| Some(&n.id) != b.forked_from.as_ref()) {
                        own.push(n.clone());
                        cur = n.parent.as_ref().and_then(|p| nodes.get(p));
                    }
                }
                own
            })?;
            Ok(json!({ "nodes": own.iter().map(window::wire).collect::<Vec<_>>() }))
        }

        "session.seen" => {
            let session: SessionId = str_param(p, "session")?.into();
            Ok(json!({ "session": kernel.mark_seen(&session)? }))
        }

        "session.retitle" => {
            let session: SessionId = str_param(p, "session")?.into();
            kernel.retitle(&session).await?;
            Ok(json!({}))
        }

        "session.rename" => {
            let session: SessionId = str_param(p, "session")?.into();
            let title = str_param(p, "title")?;
            kernel.set_title(&session, title)?;
            Ok(json!({}))
        }

        "session.delete" => {
            let session: SessionId = str_param(p, "session")?.into();
            kernel.delete_session(&session)?;
            Ok(json!({}))
        }

        "session.fork" => {
            let node: NodeId = str_param(p, "node")?.into();
            let session = kernel.fork_session(&node)?;
            Ok(json!({ "session": session }))
        }

        // The state the branch's path leaves (workspaces, model, …). Its nodes
        // are already the client's: `session.tree` and the pushes carry them.
        "branch.path" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let (_, state) = kernel.branch_path(&branch)?;
            Ok(json!({ "state": state }))
        }

        "prompt" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            // `attachments` (optional): stored-blob references, resolved
            // server-side into full attachments — the wire is trusted for the
            // hash and display name only. An unknown hash is a request error.
            // Text may be empty when images ride along.
            let mut attachments = Vec::new();
            for a in p
                .get("attachments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let hash = a
                    .get("hash")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ServerError::MissingParam("attachments[].hash".into()))?;
                let name = a.get("name").and_then(Value::as_str).map(str::to_owned);
                attachments.push(state.blobs.describe(hash, name)?);
            }
            let text = p.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.is_empty() && attachments.is_empty() {
                return Err(ServerError::MissingParam("text".into()));
            }
            // Each image also becomes a file in the session's `@scratch`, the
            // path its projected label names, so tools can work on it.
            if !attachments.is_empty() {
                let session = kernel.session_of(&branch)?;
                let root = state.workspaces.scratch(&session).root;
                let scratch = std::path::Path::new(root.as_str());
                for a in &attachments {
                    state
                        .blobs
                        .copy_to(&a.hash, &scratch.join(a.scratch_path()))?;
                }
            }
            let content = Content::text(text).with_attachments(attachments);
            // `model` (optional): the composer's pick, recorded where it
            // changes the branch.
            kernel.apply_choice(&branch, parse_params(p)?).await?;
            kernel.prompt(&branch, content).await?;
            Ok(json!({}))
        }

        // Stop this branch's reply and nothing else: background tasks it
        // started keep running (the web's stop button), and the user's
        // steering prompts not written yet come back for the composer, as
        // `prompt.cancel` gives them. `abort` is the blunter one — it cancels
        // the tasks too.
        "branch.stop" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let prompts: Vec<Value> = kernel
                .stop(&branch)?
                .into_iter()
                .map(|(node, content)| json!({ "node": node, "content": content }))
                .collect();
            Ok(json!({ "prompts": prompts }))
        }

        "abort" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.abort(&branch)?;
            Ok(json!({}))
        }

        // Take back a steering prompt (`prompt.queued`) the running turn has
        // not written yet; its content comes back for the composer.
        "prompt.cancel" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let node: NodeId = str_param(p, "node")?.into();
            let content = kernel.cancel_queued(&branch, &node)?;
            Ok(json!({ "content": content }))
        }

        // Resolves once nothing more happens on the branch until it is
        // prompted (see `Kernel::is_idle`), right after a `branch.idle` push.
        "branch.wait_idle" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.wait_idle(&branch).await?;
            Ok(json!({}))
        }

        // Make the registered workspace `name` the branch's default, telling
        // the model `text`.
        "branch.use_workspace" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let name = str_param(p, "name")?;
            if state.workspaces.get(name).is_none() {
                return Err(ServerError::InvalidParams(format!("no workspace `{name}`")));
            }
            let text = str_param(p, "text")?;
            kernel
                .use_workspace(&branch, name.to_owned(), text.to_owned())
                .await?;
            Ok(json!({}))
        }

        // Move the main line `branch` to `node`: an edit's starting point,
        // another variant's end. With `stop`, background tasks and
        // sub-agents of the part it leaves are stopped rather than refusing.
        "branch.move" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let node: NodeId = str_param(p, "node")?.into();
            let branch = kernel.move_head(&branch, &node, stop_param(p)).await?;
            Ok(json!({ "branch": branch }))
        }

        // A side question off the main line `branch`; `quote` (optional) is
        // the passage `{ node, text }` it is about.
        "branch.side" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let text = str_param(p, "text")?;
            let quote = match p.get("quote") {
                Some(q) if !q.is_null() => Some(parse_params(q)?),
                _ => None,
            };
            let side = kernel.side(&branch, quote, Content::text(text)).await?;
            Ok(json!({ "branch": side }))
        }

        "branch.regenerate" => {
            let node: NodeId = str_param(p, "node")?.into();
            let branch = kernel
                .regenerate(&node, parse_params(p)?, stop_param(p))
                .await?;
            Ok(json!({ "branch": branch }))
        }

        // What reverting the file changes since the user message `node` on
        // `branch` would do, file by file (nothing is changed).
        "branch.revert_plan" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let node: NodeId = str_param(p, "node")?.into();
            let nodes = kernel.nodes_since(&branch, &node)?;
            let (_, path_state) = kernel.branch_path(&branch)?;
            Ok(json!(rumu_plugins::revert_plan(&path_state, &nodes).await))
        }

        // Rewind to the user message `node` on `branch`: revert the picked
        // `files` ({ws, path}) of its plan, planned afresh, and with
        // `conversation` move the branch to just before the message (an
        // edit's move; what it leaves stays a variant). The model is told on
        // the branch. Refused while `branch` works.
        "branch.rewind" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let node: NodeId = str_param(p, "node")?.into();
            let files: Vec<rumu_core::FileRef> =
                parse_params(p.get("files").unwrap_or(&Value::Null))?;
            let conversation = p.get("conversation").and_then(Value::as_bool) == Some(true);
            // A move is checked (and with `stop` cleared) by `move_head`.
            if !conversation && !kernel.is_idle(&branch)? {
                return Err(ServerError::InvalidParams(
                    "the branch is still working".into(),
                ));
            }
            let nodes = kernel.nodes_since(&branch, &node)?;
            let Some((first, rumu_core::NodeBody::User { content })) =
                nodes.first().map(|n| (n, &n.body))
            else {
                return Err(ServerError::InvalidParams(format!(
                    "`{node}` is not a user message"
                )));
            };
            // The files are reverted on the path they were written on.
            let (_, path_state) = kernel.branch_path(&branch)?;
            let moved = match (conversation, &first.parent) {
                (true, Some(before)) => {
                    Some(kernel.move_head(&branch, before, stop_param(p)).await?)
                }
                (true, None) => {
                    return Err(ServerError::InvalidParams(
                        "the first message has nothing before it".into(),
                    ));
                }
                (false, _) => None,
            };
            let (plan, done) = rumu_plugins::revert_files(&path_state, &nodes, &files).await;
            let reverted: Vec<rumu_core::FileRef> = done
                .iter()
                .filter(|f| f.error.is_none())
                .map(|f| rumu_core::FileRef {
                    ws: f.ws.clone(),
                    path: f.path.clone(),
                })
                .collect();
            let text = rumu_plugins::revert_note(&content.text, moved.is_some(), &plan, &done);
            if let Some(text) = text.filter(|_| moved.is_some() || !reverted.is_empty()) {
                let marker = rumu_core::FilesReverted {
                    since: first.at,
                    files: reverted,
                };
                kernel.record_revert(&branch, text, &marker).await?;
            }
            Ok(json!({ "files": done, "branch": moved }))
        }

        "branch.retry" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.retry(&branch, parse_params(p)?).await?;
            Ok(json!({}))
        }

        "branch.compact" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let focus = p.get("focus").and_then(Value::as_str).map(str::to_owned);
            kernel.compact(&branch, focus).await?;
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

        // Cancel every background task of a branch and its sub-agents (the
        // task tab's "stop all"); the branch's reply keeps going.
        "task.kill_all" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.abort_tasks(&branch, false, rumu_kernel::CancelSource::User);
            Ok(json!({}))
        }

        // The cards a branch sees (its mount table, `deck.mount` included), or
        // without a `branch` what an assistant deck sees by default:
        // `/cards/self` plus its `deck.yaml` mounts.
        "card.list" => {
            let cards = Cards::new(&state.decks, &card_mounts(state, p)?);
            let out: Vec<rumu_cards::CardInfo> = match cards.all() {
                Ok(all) => all
                    .into_values()
                    .map(|loaded| card_info(&cards, loaded))
                    .collect(),
                Err(e) => {
                    warn!(error = %e, "card.list failed");
                    vec![]
                }
            };
            Ok(serde_json::to_value(out)?)
        }

        // One card by its mounted id (`self/dev/x`) as `card.list` lists it,
        // or null when no card is there (a folder, or gone).
        "card.get" => {
            let cards = Cards::new(&state.decks, &card_mounts(state, p)?);
            let loaded = cards.load(str_param(p, "id")?)?;
            Ok(json!(loaded.map(|l| card_info(&cards, l))))
        }

        // ---- the library (the user's view of every deck) ----
        "library.decks" => Ok(json!(library::decks(&state.decks)?)),

        "library.deck_create" => {
            let name = str_param(p, "name")?;
            Ok(json!(library::deck_create(&state.decks, name)?))
        }

        "library.deck_rename" => {
            let id = str_param(p, "id")?;
            let name = str_param(p, "name")?;
            let deck = deck_param(state, id)?;
            Ok(json!(library::deck_rename(&state.decks, &deck, name)?))
        }

        "library.deck_delete" => {
            let deck = deck_param(state, str_param(p, "id")?)?;
            state.decks.delete(deck.id())?;
            rumu_plugins::drop_project(&state.home, &state.workspaces, deck.id())?;
            Ok(json!({}))
        }

        "library.move" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Move {
                deck: String,
                from: String,
                to: String,
                #[serde(default)]
                folder: bool,
            }
            let req: Move = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let deck = deck_param(state, &req.deck)?;
            library::move_path(&state.decks, &deck, &req.from, &req.to, req.folder)?;
            Ok(json!({}))
        }

        "library.delete_folder" => {
            let deck = deck_param(state, str_param(p, "deck")?)?;
            library::delete_folder(&state.decks, &deck, str_param(p, "path")?)?;
            Ok(json!({}))
        }

        "library.cards" => {
            let deck = optional_deck_param(state, p)?;
            Ok(json!(library::cards(&state.decks, deck.as_ref())?))
        }

        "library.search" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Search {
                query: String,
                #[serde(default)]
                deck: Option<String>,
            }
            let req: Search = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            if req.query.trim().is_empty() {
                return Err(ServerError::MissingParam("query".into()));
            }
            let deck = match req.deck.as_deref().filter(|deck| !deck.is_empty()) {
                Some(id) => Some(deck_param(state, id)?),
                None => None,
            };
            Ok(json!(library::search(
                &state.decks,
                &req.query,
                deck.as_ref()
            )?))
        }

        "library.card" => {
            let deck = deck_param(state, str_param(p, "deck")?)?;
            let path = str_param(p, "path")?;
            Ok(json!(library::detail(&state.decks, &deck, path)?))
        }

        "library.save" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct RefReq {
                deck: String,
                path: String,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Save {
                deck: String,
                path: String,
                view: String,
                base: String,
                #[serde(default)]
                from: Option<RefReq>,
            }
            let req: Save = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let deck = deck_param(state, &req.deck)?;
            let from_deck = match &req.from {
                Some(from) => Some(deck_param(state, &from.deck)?),
                None => None,
            };
            let from = match (&req.from, &from_deck) {
                (Some(from), Some(from_deck)) => Some((from_deck, from.path.as_str())),
                _ => None,
            };
            Ok(json!(library::save(
                &state.decks,
                &deck,
                library::SaveRequest {
                    path: &req.path,
                    view: &req.view,
                    base: &req.base,
                    from,
                },
            )?))
        }

        "library.delete" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Delete {
                deck: String,
                path: String,
                base: String,
            }
            let req: Delete = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let deck = deck_param(state, &req.deck)?;
            library::delete(&state.decks, &deck, &req.path, &req.base)?;
            Ok(json!({}))
        }

        "library.history" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct History {
                deck: String,
                path: String,
            }
            let req: History = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let deck = deck_param(state, &req.deck)?;
            Ok(json!(library::history(&state.decks, &deck, &req.path)?))
        }

        "library.version" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Version {
                deck: String,
                path: String,
                commit: String,
            }
            let req: Version = serde_json::from_value(p.clone())
                .map_err(|e| ServerError::InvalidParams(e.to_string()))?;
            let deck = deck_param(state, &req.deck)?;
            Ok(json!(library::version(
                &state.decks,
                &deck,
                &req.path,
                &req.commit
            )?))
        }

        other => Err(ServerError::UnknownMethod(other.to_owned())),
    }
}

/// The mount table `card.list` / `card.get` read through: the `branch`'s,
/// or without one the `assistant` deck's default (`/cards/self` plus its
/// `deck.yaml` mounts).
fn card_mounts(state: &AppState, p: &Value) -> Result<Vec<CardMount>> {
    if let Some(branch) = p.get("branch").and_then(Value::as_str) {
        return Ok(state.kernel.branch_path(&BranchId::from(branch))?.1.mounts);
    }
    let deck = assistant_param(state, p)?;
    let mut mounts = vec![CardMount::new(
        deck.id(),
        rumu_cards::cards::SELF_MOUNT,
        false,
    )];
    mounts.extend(state.decks.default_mounts(deck.id()));
    Ok(mounts)
}

/// A loaded card as the web lists it.
fn card_info(cards: &Cards, loaded: rumu_cards::cards::Loaded) -> rumu_cards::CardInfo {
    // The card file's own deck and path (for a link, not the target's): the
    // drawer opens it in the library.
    let (deck, path) = cards
        .mount_of(&loaded.path)
        .map(|(mounted, path)| (mounted.deck.id().to_owned(), path.to_owned()))
        .unwrap_or_default();
    let link = loaded
        .link
        .clone()
        .or_else(|| loaded.card.link().map(str::to_owned));
    rumu_cards::CardInfo {
        id: loaded.path,
        deck,
        path,
        description: loaded.card.summary(),
        keywords: loaded
            .card
            .keywords()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        pinned: loaded.card.pinned(),
        body: loaded.card.body,
        revision: loaded.card.revision,
        link,
        meta: loaded.card.meta,
    }
}

fn str_param<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ServerError::MissingParam(key.to_owned()))
}

/// Token totals over some model calls.
#[derive(Default, Serialize)]
struct UsageTotals {
    calls: u64,
    input: u64,
    cached: u64,
    output: u64,
}

impl UsageTotals {
    fn add(&mut self, n: &rumu_core::Node) {
        if let rumu_core::NodeBody::Assistant { usage, .. } = &n.body {
            self.calls += 1;
            self.input += usage.prompt_tokens;
            self.cached += usage.cached_tokens;
            self.output += usage.completion_tokens;
        }
    }
}

/// Token totals over every model call in `nodes`.
fn session_usage<'a>(nodes: impl IntoIterator<Item = &'a rumu_core::Node>) -> UsageTotals {
    let mut t = UsageTotals::default();
    nodes.into_iter().for_each(|n| t.add(n));
    t
}

/// Each sub-agent's totals over its own nodes (head up to its fork point),
/// so the stats view lists them without fetching their nodes.
fn agent_usage(
    nodes: &HashMap<NodeId, rumu_core::Node>,
    branches: &HashMap<BranchId, rumu_core::Branch>,
) -> HashMap<BranchId, UsageTotals> {
    let mut out = HashMap::new();
    for b in branches.values() {
        if !matches!(b.kind, rumu_core::BranchKind::Agent { .. }) {
            continue;
        }
        let mut t = UsageTotals::default();
        let mut cur = Some(&b.head);
        while let Some(n) = cur
            .filter(|id| Some(*id) != b.forked_from.as_ref())
            .and_then(|id| nodes.get(id))
        {
            t.add(n);
            cur = n.parent.as_ref();
        }
        out.insert(b.id.clone(), t);
    }
    out
}

/// The project deck `id`; any other id is a bad request.
fn project_deck(state: &AppState, id: &str) -> Result<rumu_cards::Deck> {
    state
        .decks
        .get(id)
        .filter(|d| d.project())
        .ok_or_else(|| ServerError::InvalidParams(format!("no project deck `{id}`")))
}

/// The optional `project` param: a project deck id, or none when omitted or
/// null.
fn project_param(state: &AppState, p: &Value) -> Result<Option<rumu_cards::Deck>> {
    match p.get("project") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) => project_deck(state, id).map(Some),
        Some(_) => Err(ServerError::InvalidParams(
            "`project` must be a deck id".into(),
        )),
    }
}

/// The deck a library request names; an unknown id is a bad request.
fn deck_param(state: &AppState, id: &str) -> Result<rumu_cards::Deck> {
    state
        .decks
        .get(id)
        .ok_or_else(|| ServerError::InvalidParams(format!("unknown deck `{id}`")))
}

/// The optional `deck` filter of a library listing.
fn optional_deck_param(state: &AppState, p: &Value) -> Result<Option<rumu_cards::Deck>> {
    match p
        .get("deck")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        Some(id) => Ok(Some(deck_param(state, id)?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_append(id: &str, text: &str, end: u64) -> Value {
        json!({ "method": "task.progress", "params": { "id": id, "branch": "b", "append": text, "end": end } })
    }

    /// Appends gather per task until something else goes out, which sends
    /// them first; a watched task skips what its buffer already held and an
    /// unwatched one sends no output, only its other updates.
    #[test]
    fn feed_gathers_appends_and_filters_tasks() {
        let mut feed = Feed {
            tasks: Some(HashMap::from([("t1".to_owned(), 3)])),
            ..Feed::default()
        };
        assert!(feed.take(task_append("t1", "abc", 3)).is_empty()); // held
        assert!(feed.take(task_append("t1", "de", 5)).is_empty());
        assert!(feed.take(task_append("t1", "f", 6)).is_empty());
        assert!(feed.take(task_append("t2", "zz", 2)).is_empty()); // unwatched
        assert!(feed.due.is_some());
        let data = json!({ "method": "task.progress", "params": { "id": "t2", "branch": "b", "append": "y", "end": 3, "data": 1 } });
        let out = feed.take(data);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["params"]["append"], "def");
        assert_eq!(out[0]["params"]["end"], 6);
        assert_eq!(
            out[1]["params"],
            json!({ "id": "t2", "branch": "b", "data": 1 })
        );
        assert!(feed.due.is_none() && feed.pending.is_empty());
    }

    /// A flood keeps only the tail of what gathered, cut on a char boundary.
    #[test]
    fn feed_keeps_the_tail_of_a_flood() {
        let mut feed = Feed::default();
        let push = |s: String| json!({ "method": "tool.progress", "params": { "branch": "b", "call_id": "c", "append": s } });
        feed.take(push("é".repeat(PROGRESS_TAIL)));
        feed.take(push("end".to_owned()));
        let out = feed.flush();
        let text = out[0]["params"]["append"].as_str().unwrap();
        assert!(text.len() <= PROGRESS_TAIL && text.ends_with("éend"));
    }

    /// A branch's same-kind deltas go out as one push carrying the first
    /// `at`, the last `seq` and the first as `from`; a kind switch starts a
    /// new one, other branches gather apart, and anything else sends them
    /// first.
    #[test]
    fn feed_merges_model_deltas() {
        let delta = |b: &str, k: &str, s: &str, seq: u64, at: u64| json!({ "method": "llm.delta", "params": { "branch": b, k: s, "seq": seq, "at": at } });
        let mut feed = Feed::default();
        assert!(feed.take(delta("a", "reasoning", "th", 1, 0)).is_empty());
        assert!(feed.take(delta("b", "text", "x", 1, 0)).is_empty());
        assert!(feed.take(delta("a", "reasoning", "ink", 2, 2)).is_empty());
        assert!(feed.take(delta("a", "text", "hi", 3, 0)).is_empty());
        assert!(feed.take(delta("a", "text", "!", 4, 2)).is_empty());
        assert!(feed.due.is_some());
        let out = feed.take(json!({ "method": "node.appended", "params": {} }));
        let params: Vec<_> = out.iter().map(|v| v["params"].clone()).collect();
        assert_eq!(
            params,
            [
                json!({ "branch": "a", "reasoning": "think", "seq": 2, "at": 0, "from": 1 }),
                json!({ "branch": "b", "text": "x", "seq": 1, "at": 0 }),
                json!({ "branch": "a", "text": "hi!", "seq": 4, "at": 0, "from": 3 }),
                json!({}),
            ]
        );
    }
}
