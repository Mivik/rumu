//! WebSocket protocol, blob routes and static serving.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
    BranchId, CardMount, CardUnmount, Content, NodeId, SessionId, SessionRoot, TaskId, Workspace,
    Workspaces,
};
use rumu_kernel::Kernel;
use rumu_plugins::{Assistant, assistant_of};

use crate::settings::{Settings, SettingsError};
use rust_embed::{EmbeddedFile, RustEmbed};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;
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

    /// A workspace file could not be opened (`fs.snapshot`).
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
}

impl ServerError {
    /// The JSON-RPC 2.0 error code.
    fn code(&self) -> i64 {
        match self {
            Self::UnknownMethod(_) => -32601,
            Self::MissingParam(_) | Self::InvalidParams(_) => -32602,
            // A stale write: the web shows its "this card changed" banner.
            Self::Cards(rumu_cards::Error::Stale { .. }) => -32010,
            _ => -32000,
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
        .route("/blobs/{hash}", get(blob_get));
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

    // `session.watch`: the session this client shows (`Some(None)`: none).
    // Until it is called, the client gets every push.
    let mut watched: Option<Option<SessionId>> = None;
    let mut sub = state.kernel.subscribe();
    loop {
        tokio::select! {
            push = sub.recv() => match push {
                Ok(v) => {
                    if let Some(watched) = &watched
                        && push_session(&state.kernel, &v).is_some_and(|s| Some(&s) != watched.as_ref())
                    {
                        continue;
                    }
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
                    // Answered in order, not spawned: the pushes after the
                    // reply, and the requests after it, see the new filter.
                    if let Some((id, session)) = watch_request(text.as_str()) {
                        watched = Some(session);
                        let reply = json!({ "jsonrpc": "2.0", "id": id, "result": null });
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

/// A `session.watch { session: id | null }` request: its id and session.
fn watch_request(raw: &str) -> Option<(Value, Option<SessionId>)> {
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
    }
    let req: Req = serde_json::from_str(raw).ok()?;
    (req.method == "session.watch").then_some((req.id, req.params.session))
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
    match call(state, &method, &params).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string(),
        Err((code, message)) => error_frame(id, code, message).to_string(),
    }
}

/// Run one request as a WebSocket client would: its result, or the JSON-RPC
/// error code and message.
pub(crate) async fn call(
    state: &AppState,
    method: &str,
    params: &Value,
) -> std::result::Result<Value, (i64, String)> {
    dispatch(state, method, params)
        .await
        .map_err(|e| (e.code(), e.to_string()))
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
fn error_frame(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
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
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
            let raw = p.get("path").and_then(Value::as_str).unwrap_or("~");
            let path = match raw.strip_prefix('~') {
                Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
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

        // `agents: false` leaves out the nodes only sub-agent branches reach
        // (most of a long session's bytes); `branch.nodes` fetches them.
        // `usage` totals every model call of the session either way.
        "session.tree" => {
            let session: SessionId = str_param(p, "session")?.into();
            let agents = p.get("agents").and_then(Value::as_bool).unwrap_or(true);
            let (session, mut nodes, branches) = kernel.session_tree(&session)?;
            let usage = session_usage(&nodes);
            if !agents {
                let lines = line_nodes(&nodes, &branches);
                nodes.retain(|n| lines.contains(&n.id));
            }
            let live = kernel.live_turns(&session.id)?;
            Ok(json!({
                "session": session,
                "nodes": nodes,
                "branches": branches,
                "live": live,
                "usage": usage,
            }))
        }

        // The nodes of `branch`'s path that `session.tree` with
        // `agents: false` leaves out, root → head (none for a main line or a
        // side question).
        "branch.nodes" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let session = kernel.session_of(&branch)?;
            let (_, nodes, branches) = kernel.session_tree(&session)?;
            let lines = line_nodes(&nodes, &branches);
            let head = branches.iter().find(|b| b.id == branch).map(|b| &b.head);
            let by_id: HashMap<&NodeId, &rumu_core::Node> =
                nodes.iter().map(|n| (&n.id, n)).collect();
            let mut own = Vec::new();
            let mut cur = head;
            while let Some(n) = cur.and_then(|id| by_id.get(id)) {
                if lines.contains(&n.id) {
                    break;
                }
                own.push(*n);
                cur = n.parent.as_ref();
            }
            own.reverse();
            Ok(json!({ "nodes": own }))
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
        // started keep running (the web's stop button). `abort` is the blunter
        // one — it cancels them too.
        "branch.stop" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            kernel.stop(&branch)?;
            Ok(json!({}))
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
        // another variant's end.
        "branch.move" => {
            let branch: BranchId = str_param(p, "branch")?.into();
            let node: NodeId = str_param(p, "node")?.into();
            let branch = kernel.move_head(&branch, &node).await?;
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
            let branch = kernel.regenerate(&node, parse_params(p)?).await?;
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
            if !kernel.is_idle(&branch)? {
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
                (true, Some(before)) => Some(kernel.move_head(&branch, before).await?),
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
            kernel.abort_tasks(&branch, false);
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

/// The nodes no sub-agent added: the main line with every variant it left
/// (the variant switcher walks them all) and the side questions. A
/// sub-agent's own nodes run from its head up to the node it forked from.
fn line_nodes(nodes: &[rumu_core::Node], branches: &[rumu_core::Branch]) -> HashSet<NodeId> {
    let parent: HashMap<&NodeId, Option<&NodeId>> =
        nodes.iter().map(|n| (&n.id, n.parent.as_ref())).collect();
    let mut agents = HashSet::new();
    for b in branches {
        if !matches!(b.kind, rumu_core::BranchKind::Agent { .. }) {
            continue;
        }
        let mut cur = Some(&b.head);
        while let Some(id) = cur.filter(|id| Some(*id) != b.forked_from.as_ref()) {
            agents.insert(id);
            cur = parent.get(id).copied().flatten();
        }
    }
    nodes
        .iter()
        .map(|n| &n.id)
        .filter(|id| !agents.contains(id))
        .cloned()
        .collect()
}

/// Token totals over every model call in `nodes`.
fn session_usage(nodes: &[rumu_core::Node]) -> Value {
    let (mut calls, mut input, mut cached, mut output) = (0u64, 0u64, 0u64, 0u64);
    for n in nodes {
        if let rumu_core::NodeBody::Assistant { usage, .. } = &n.body {
            calls += 1;
            input += usage.prompt_tokens;
            cached += usage.cached_tokens;
            output += usage.completion_tokens;
        }
    }
    json!({ "calls": calls, "input": input, "cached": cached, "output": output })
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
    use rumu_core::{Branch, BranchKind, BranchStatus, Content, Node, NodeBody};

    use super::*;

    fn node(id: &str, parent: Option<&str>) -> Node {
        Node {
            id: id.into(),
            parent: parent.map(Into::into),
            session: "s".into(),
            at: 0,
            dur_ms: None,
            body: NodeBody::User {
                content: Content::text(id),
            },
        }
    }

    fn branch(id: &str, head: &str, forked_from: Option<&str>, kind: BranchKind) -> Branch {
        Branch {
            id: id.into(),
            session: "s".into(),
            head: head.into(),
            forked_from: forked_from.map(Into::into),
            kind,
            status: BranchStatus::Active,
            created_at: 0,
        }
    }

    /// The main line (every variant) and side questions stay; what a
    /// sub-agent added, nested ones included, is left out.
    #[test]
    fn line_nodes_leave_out_only_sub_agents() {
        let nodes = vec![
            node("r", None),
            node("a", Some("r")),
            node("v", Some("r")), // a variant of `a`
            node("q", Some("a")), // a side question off `a`
            node("g1", Some("a")),
            node("g2", Some("g1")),
            node("n1", Some("g2")), // a sub-agent of the sub-agent
        ];
        let agent = |parent: &str| BranchKind::Agent {
            task: String::new(),
            parent_branch: parent.into(),
            call_id: "c".into(),
            via_plugin: false,
            inherit: Default::default(),
        };
        let branches = vec![
            branch("main", "a", None, BranchKind::Main),
            branch(
                "side",
                "q",
                Some("a"),
                BranchKind::Side {
                    parent_branch: "main".into(),
                    quote: None,
                },
            ),
            branch("g", "g2", Some("a"), agent("main")),
            branch("n", "n1", Some("g2"), agent("g")),
        ];
        let mut kept: Vec<_> = line_nodes(&nodes, &branches)
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        kept.sort();
        assert_eq!(kept, ["a", "q", "r", "v"]);
    }
}
