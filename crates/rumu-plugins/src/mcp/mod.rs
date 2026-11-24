//! MCP: servers registered in `settings.yaml` (see [`McpHub`]), bound to
//! assistants and projects by name. Their tools stay out of the model's
//! tool list: each server is an object under `@rumu:/mcp` whose actions are
//! its tools (`<server>/<tool>`). The model finds them with `list`, `read`
//! and `search` there and runs one with `invoke`, all served by the
//! workspace plugin through the functions here. See `docs/mcp.md`.

mod hub;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use rmcp::model::{JsonObject, Tool};
use rumu_cards::Decks;
use rumu_cards::recall::{Doc, rank_docs};
use rumu_core::{
    BranchId, Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, McpApp, McpAppContext,
    McpAppCsp, McpStatus, PathState, Plugin, RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured,
    ToolError, ToolOutput, ToolResult, any_rules_enable,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tracing::{info, warn};

use crate::workspace::vfs::{actions_section, signature};

pub use hub::{McpHub, Server, Snapshot};

/// A server's `instructions` in the first-call note are cut to this many
/// characters.
const INSTRUCTIONS_LIMIT: usize = 2_000;
/// Default `search` hits under `@rumu:/mcp`: each prints its schema.
pub const SEARCH_LIMIT: usize = 5;
/// Resources (and resource templates) a server's listing shows by URI; the
/// rest are counted.
const RESOURCES_SHOWN: usize = 100;
const INTRO_HOOK: &str = "mcp-intro";
/// The MCP Apps extension, and the kind of view rumu renders.
pub const UI_EXTENSION: &str = "io.modelcontextprotocol/ui";
pub const APP_MIME: &str = "text/html;profile=mcp-app";
/// What a note keeps of a view's model context, in characters.
const CONTEXT_LIMIT: usize = 4_000;

/// Payload of the `mcp.intro` note (v1): the server list, again whenever it
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct McpIntro {
    pub servers: Vec<String>,
    /// Each configured server's state when the note was written, for the
    /// UI; absent in notes written before it existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub status: BTreeMap<String, McpIntroServer>,
}

/// A server's line in the `mcp.intro` note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct McpIntroServer {
    pub status: McpStatus,
    /// Enabled tools (when ready).
    pub tools: usize,
    /// The first line of the error (when failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

impl Structured for McpIntro {
    const TAG: &'static str = "mcp.intro";
    const V: u32 = 1;
}

/// The server-list note. The servers' tools run through `invoke` (the
/// workspace plugin, with [`call`]).
pub struct McpPlugin {
    hub: Arc<McpHub>,
    decks: Decks,
}

impl McpPlugin {
    pub fn new(hub: Arc<McpHub>, decks: Decks) -> Self {
        Self { hub, decks }
    }
}

#[async_trait]
impl Plugin for McpPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mcp".into(),
            instructions: None,
            tools: vec![],
            hooks: vec![HookSpec {
                id: INTRO_HOOK.into(),
                matcher: json!({ "event": "llm.pre" }),
                priority: i32::MAX - 3,
            }],
        }
    }

    async fn on_hook(
        &self,
        hook_id: &str,
        _event: &Event,
        cx: &Cx,
    ) -> rumu_core::Result<HookOutput> {
        if hook_id != INTRO_HOOK {
            return Ok(HookOutput::default());
        }
        let mut effects: Vec<Content> = self.servers_note(cx).await?.into_iter().collect();
        effects.extend(self.hub.take_contexts(&cx.branch));
        Ok(HookOutput {
            effects,
            decision: Decision::Pass,
        })
    }
}

impl McpPlugin {
    /// The server-list note, when the list changed since the last one.
    async fn servers_note(&self, cx: &Cx) -> rumu_core::Result<Option<Content>> {
        let state = cx.state()?;
        let mcp = servers(&self.decks, &state);
        let servers: Vec<String> = mcp.keys().cloned().collect();
        let last = cx.index()?.all::<McpIntro>().pop().map(|e| e.value.servers);
        if last.as_ref().unwrap_or(&Vec::new()) == &servers {
            return Ok(None);
        }
        let (text, status) = match (last, servers.is_empty()) {
            (_, true) => (
                "This session has no MCP servers any more.".to_owned(),
                BTreeMap::new(),
            ),
            (None, false) => intro(&self.hub, &mcp).await,
            (Some(_), false) => {
                let (text, status) = intro(&self.hub, &mcp).await;
                (
                    format!("The MCP servers changed; this replaces the earlier list.\n{text}"),
                    status,
                )
            }
        };
        Ok(Some(Content::note_with(
            text,
            &McpIntro { servers, status },
        )))
    }
}

/// `invoke` of `<name>/<tool>`: run the tool of one of the session's
/// servers (`mcp`) and turn its result into a tool output. A call the server
/// refuses comes back with the tool's input schema. A tool with a view
/// (MCP Apps) has its page read along with the call, and the output
/// carries both for the web ([`McpApp`]).
pub async fn call(
    hub: &McpHub,
    mcp: &Servers,
    state: &PathState,
    blobs: impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
    name: &str,
    tool: &str,
    args: JsonObject,
) -> ToolResult<ToolOutput> {
    let (server, snap) = bound(hub, mcp, name).await?;
    let Some(spec) = snap.tools.iter().find(|t| t.name == tool) else {
        return Err(ToolError::model(format!(
            "`{name}` has no tool `{tool}`; its tools: {}",
            names(&snap.tools)
        )));
    };
    let ui = ui_resource(spec);
    let page = async {
        match ui {
            Some(uri) => Some(server.read_resource(uri).await),
            None => None,
        }
    };
    let (result, page) = tokio::join!(server.call(tool, args.clone()), page);
    let result = result.map_err(|e| {
        let schema = Value::Object((*spec.input_schema).clone());
        ToolError::model(format!("{name}/{tool}: {e}; its arguments: {schema}"))
    })?;
    let out = output(&result, name, scratch(state), &blobs).await?;
    let (Some(uri), Some(page)) = (ui, page) else {
        return Ok(out);
    };
    match page.and_then(|page| app_view((name, tool, uri), &page, &args, &result, &blobs)) {
        Ok(app) => {
            let mut out = out.with_data(&app);
            out.content
                .text
                .push_str("\n[The user sees this result in an interactive view.]");
            Ok(out)
        }
        Err(e) => {
            warn!(
                server = name,
                tool,
                uri,
                error = e,
                "mcp app view unavailable"
            );
            Ok(out)
        }
    }
}

/// The `ui://` resource `tool` shows its results in: `_meta.ui.resourceUri`,
/// or the deprecated flat `_meta["ui/resourceUri"]`.
fn ui_resource(tool: &Tool) -> Option<&str> {
    let meta = tool.meta.as_deref()?;
    meta.get("ui")
        .and_then(|ui| ui.get("resourceUri"))
        .or_else(|| meta.get("ui/resourceUri"))
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("ui://"))
}

/// Whether `tool` is for `who` (`model` or `app`): its `_meta.ui.visibility`,
/// both when it has none.
fn visible_to(tool: &Tool, who: &str) -> bool {
    tool.meta
        .as_deref()
        .and_then(|m| m.get("ui"))
        .and_then(|ui| ui.get("visibility"))
        .and_then(Value::as_array)
        .is_none_or(|v| v.iter().any(|w| w == who))
}

/// The view of a call of `(server, tool)` showing `uri`: `page`, what
/// `resources/read` of it returned, and the call, stored as blobs.
fn app_view(
    (server, tool, uri): (&str, &str, &str),
    page: &rmcp::model::ReadResourceResult,
    args: &JsonObject,
    result: &Value,
    blobs: &impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
) -> Result<McpApp, String> {
    let contents = serde_json::to_value(&page.contents).map_err(|e| e.to_string())?;
    let contents = contents.as_array().map(Vec::as_slice).unwrap_or_default();
    let item = contents
        .iter()
        .find(|c| c["uri"] == uri)
        .or(contents.first())
        .ok_or("the resource is empty")?;
    if let Some(mime) = item["mimeType"].as_str()
        && !mime.starts_with("text/html")
    {
        return Err(format!("the resource is {mime}, not HTML"));
    }
    let html = match (item["text"].as_str(), item["blob"].as_str()) {
        (Some(text), _) => text.as_bytes().to_vec(),
        (None, Some(data)) => base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| format!("bad base64 data: {e}"))?,
        _ => return Err("the resource has no content".into()),
    };
    let call = json!({ "arguments": args, "result": result });
    let blobs = blobs().map_err(|e| e.to_string())?;
    let put = |bytes: &[u8]| blobs.put(bytes).map_err(|e| e.to_string());
    let ui = &item["_meta"]["ui"];
    let csp = &ui["csp"];
    Ok(McpApp {
        server: server.to_owned(),
        tool: tool.to_owned(),
        uri: uri.to_owned(),
        html: put(&html)?,
        call: put(call.to_string().as_bytes())?,
        csp: McpAppCsp {
            connect: sources(&csp["connectDomains"]),
            resource: sources(&csp["resourceDomains"]),
            frame: sources(&csp["frameDomains"]),
            base_uri: sources(&csp["baseUriDomains"]),
        },
        prefers_border: ui["prefersBorder"].as_bool(),
    })
}

/// A view's origins as CSP sources: an entry that could end the directive
/// or name a keyword (`'unsafe-eval'`) is dropped.
fn sources(origins: &Value) -> Vec<String> {
    origins
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-:/*_[]".contains(&b))
        })
        .map(str::to_owned)
        .collect()
}

/// `tools/call` from a view of one of `name`'s results (the web's
/// `mcp.app_call`): a tool of the same server the session's rules enable
/// and apps may call. Returns its `CallToolResult`; the model never sees it.
pub async fn app_call(
    hub: &McpHub,
    mcp: &Servers,
    name: &str,
    tool: &str,
    args: JsonObject,
) -> Result<Value, String> {
    let (server, snap) = app_server(hub, mcp, name).await?;
    if !snap
        .tools
        .iter()
        .any(|t| t.name == tool && visible_to(t, "app"))
    {
        return Err(format!("`{name}` has no tool `{tool}` for apps"));
    }
    info!(server = name, tool, "mcp app calls a tool");
    server.call(tool, args).await
}

/// `resources/read` from a view of one of `name`'s results (the web's
/// `mcp.app_read`). Returns its `ReadResourceResult`.
pub async fn app_read(hub: &McpHub, mcp: &Servers, name: &str, uri: &str) -> Result<Value, String> {
    let (server, _) = app_server(hub, mcp, name).await?;
    info!(server = name, uri, "mcp app reads a resource");
    let result = server.read_resource(uri).await?;
    serde_json::to_value(result).map_err(|e| e.to_string())
}

/// Server `name` when the session has it, with the tools its rules enable,
/// whoever they are for.
async fn app_server(
    hub: &McpHub,
    mcp: &Servers,
    name: &str,
) -> Result<(Arc<Server>, Snapshot), String> {
    let rules = mcp
        .get(name)
        .ok_or_else(|| format!("no MCP server `{name}` in this session"))?;
    let server = hub
        .server(name)
        .ok_or_else(|| format!("MCP server `{name}` is not configured"))?;
    let mut snap = server.ready().await;
    snap.tools = snap
        .tools
        .iter()
        .filter(|t| any_rules_enable(rules, &t.name))
        .cloned()
        .collect();
    Ok((server, snap))
}

/// `ui/update-model-context` from view `view` (its result node) of a call
/// of `(server, tool)` on `branch` (the web's `mcp.app_context`): kept,
/// replacing what the view said before, for the branch's next model call;
/// nothing said drops it.
pub fn app_context(
    hub: &McpHub,
    branch: &BranchId,
    view: &str,
    (server, tool): (&str, &str),
    content: &Value,
    structured: Option<&Value>,
) {
    let mut parts: Vec<String> = content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .map(str::to_owned)
        .collect();
    parts.extend(structured.filter(|s| !s.is_null()).map(Value::to_string));
    let said = parts.join("\n");
    let note = (!said.trim().is_empty()).then(|| {
        let text = format!(
            "The user's interactive view of a `{server}/{tool}` result updated what it tells \
             you (this replaces what it said before):\n{}",
            rumu_core::truncate_middle(said, CONTEXT_LIMIT)
        );
        let data = McpAppContext {
            server: server.to_owned(),
            tool: tool.to_owned(),
        };
        Content::note_with(text, &data)
    });
    hub.set_context(branch, view, note);
}

/// `mcp.read_resource` on `@rumu:/mcp/<name>`: read `uri` from one of the
/// session's servers, its contents turned into a tool output like a tool
/// result's embedded resources (several each under their URI).
pub async fn read_resource(
    hub: &McpHub,
    mcp: &Servers,
    state: &PathState,
    blobs: impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
    name: &str,
    uri: &str,
) -> ToolResult<ToolOutput> {
    let (server, _) = bound(hub, mcp, name).await?;
    let result = server
        .read_resource(uri)
        .await
        .map_err(|e| ToolError::model(format!("{name}: reading `{uri}`: {e}")))?;
    let several = result.contents.len() > 1;
    let mut content = Vec::new();
    for c in serde_json::to_value(&result.contents)
        .unwrap_or_default()
        .as_array()
        .into_iter()
        .flatten()
    {
        if several {
            let uri = c["uri"].as_str().unwrap_or("?");
            content.push(json!({ "type": "text", "text": format!("[{uri}]") }));
        }
        content.push(json!({ "type": "resource", "resource": c }));
    }
    if content.is_empty() {
        return Ok(format!("`{uri}` is empty").into());
    }
    output(&json!({ "content": content }), name, scratch(state), blobs).await
}

/// The session's `@scratch` root.
fn scratch(state: &PathState) -> Option<PathBuf> {
    state
        .workspaces
        .iter()
        .find(|w| w.name == SCRATCH_WORKSPACE)
        .map(|w| PathBuf::from(w.root.as_str()))
}

/// A `tools/call` result of server `server` as a tool output: text blocks
/// joined, images attached, and every medium (images, audio, binary
/// resources) copied into `scratch` (the session's `@scratch` root) like a
/// prompt's, the text naming the file.
async fn output(
    result: &Value,
    server: &str,
    scratch: Option<PathBuf>,
    blobs: impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
) -> ToolResult<ToolOutput> {
    let mut parts: Vec<String> = Vec::new();
    let mut images = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        let kind = block["type"].as_str().unwrap_or("");
        let (data, mime, what) = match kind {
            "text" => {
                parts.push(block["text"].as_str().unwrap_or("").to_owned());
                continue;
            }
            "image" | "audio" => (&block["data"], &block["mimeType"], kind.to_owned()),
            "resource" => {
                let r = &block["resource"];
                if let Some(text) = r["text"].as_str() {
                    parts.push(text.to_owned());
                    continue;
                }
                let what = format!("resource {}", r["uri"].as_str().unwrap_or("?"));
                (&r["blob"], &r["mimeType"], what)
            }
            "resource_link" => {
                parts.push(format!(
                    "[resource link: {}{}; read it with `invoke` `mcp.read_resource` on \
                     `{RUMU_WORKSPACE}:/mcp/{server}`]",
                    block["uri"].as_str().unwrap_or("?"),
                    block["name"]
                        .as_str()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                ));
                continue;
            }
            other => {
                parts.push(format!("[{other} content omitted]"));
                continue;
            }
        };
        let mime = mime.as_str().unwrap_or("application/octet-stream");
        let data = data.as_str().unwrap_or("");
        let (image, file) = media(data, mime, scratch.clone(), &blobs)
            .await
            .map_err(|e| ToolError::model(format!("{what}: {e}")))?;
        let attached = image.is_some();
        images.extend(image);
        match file {
            Some(file) if kind == "image" && attached => parts.push(format!(
                "[image {} saved as {SCRATCH_WORKSPACE}:{file}]",
                images.len()
            )),
            Some(file) => parts.push(format!(
                "[{what} ({mime}) saved as {SCRATCH_WORKSPACE}:{file}]"
            )),
            None if attached => {}
            None => parts.push(format!("[{what} ({mime}) omitted]")),
        }
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        parts.push(serde_json::to_string_pretty(structured).unwrap_or_default());
    }
    let text = parts.join("\n");
    let out = if result["isError"].as_bool().unwrap_or(false) {
        ToolOutput::err(text)
    } else {
        ToolOutput::ok(text)
    };
    Ok(out.with_attachments(images))
}

/// Base64 `data` of MIME type `mime`, from a server: an image rumu takes is
/// normalized and attached; anything else is kept as it is. Either is
/// copied into `scratch` when there is one. Returns the attachment and the
/// file under `scratch`.
async fn media(
    data: &str,
    mime: &str,
    scratch: Option<PathBuf>,
    blobs: &impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
) -> Result<(Option<rumu_core::Attachment>, Option<String>), String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("bad base64 data: {e}"))?;
    let blobs = match rumu_blob::is_image(&bytes) {
        true => Some(blobs().map_err(|e| format!("storing it: {e}"))?),
        false => None,
    };
    let mime = mime.to_owned();
    tokio::task::spawn_blocking(move || {
        if let Some(blobs) = blobs
            && let Ok(a) = blobs.ingest_image(&bytes, None)
        {
            let file = a.scratch_path();
            let Some(root) = scratch else {
                return Ok((Some(a), None));
            };
            blobs
                .copy_to(&a.hash, &root.join(&file))
                .map_err(|e| e.to_string())?;
            return Ok((Some(a), Some(file)));
        }
        let Some(root) = scratch else {
            return Ok((None, None));
        };
        let file = rumu_core::scratch_attachment(&rumu_blob::hash_of(&bytes), &mime);
        let path = root.join(&file);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        Ok((None, Some(file)))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A session's MCP servers, each with the rules of every deck binding it
/// (its assistant's, its project's): a tool any of them enables is enabled.
pub type Servers = BTreeMap<String, Vec<Vec<String>>>;

/// The servers of the session's assistant and of its project, with the
/// rules enabling their tools, from their decks as they are now.
pub fn servers(decks: &Decks, state: &PathState) -> Servers {
    let mut servers = Servers::new();
    for meta in crate::decks::binding(decks, state) {
        for (name, rules) in meta.mcp {
            servers.entry(name).or_default().push(rules);
        }
    }
    servers
}

/// The server-list note: the session's servers, their status and
/// instructions; and each configured server's state for the UI.
async fn intro(hub: &McpHub, mcp: &Servers) -> (String, BTreeMap<String, McpIntroServer>) {
    let mut status = BTreeMap::new();
    let mut text = format!(
        "MCP servers of this session. Their tools are not in your tool list: they are the actions \
of `{RUMU_WORKSPACE}:/mcp/<server>` — find them with `search` (scope `{RUMU_WORKSPACE}:/mcp`) or \
`list {RUMU_WORKSPACE}:/mcp/<server>`, and run one with `invoke` (`action: \"<server>/<tool>\"`, \
no `path`). That `list` also shows a server's resources; read one with `invoke` \
`mcp.read_resource` on its path."
    );
    for (name, rules) in mcp {
        let _ = write!(text, "\n- `{name}`: ");
        match hub.server(name) {
            None => text.push_str("not configured"),
            Some(server) => {
                let snap = enabled(server.ready().await, rules);
                text.push_str(&status_line(&snap));
                status.insert(
                    name.clone(),
                    McpIntroServer {
                        status: snap.status,
                        tools: snap.tools.len(),
                        error: first_error_line(&snap).map(str::to_owned),
                    },
                );
                if let Some(i) = snap
                    .instructions
                    .as_deref()
                    .map(str::trim)
                    .filter(|i| !i.is_empty())
                {
                    let i = rumu_core::truncate_middle(i.to_owned(), INSTRUCTIONS_LIMIT);
                    let _ = write!(text, "\n  {}", i.replace('\n', "\n  "));
                }
            }
        }
    }
    (text, status)
}

/// The first line of a failed server's error.
fn first_error_line(snap: &Snapshot) -> Option<&str> {
    match snap.status {
        McpStatus::Failed => snap.error.as_deref().and_then(|e| e.lines().next()),
        _ => None,
    }
}

fn status_line(snap: &Snapshot) -> String {
    match snap.status {
        McpStatus::Ready => format!("{} tools", snap.tools.len()),
        McpStatus::Connecting => "connecting".into(),
        McpStatus::Failed => format!(
            "unavailable ({})",
            first_error_line(snap).unwrap_or("failed")
        ),
    }
}

fn names(tools: &[Tool]) -> String {
    tools
        .iter()
        .map(|t| format!("`{}`", t.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `snap` with only the tools `rules` enable, and of them only the ones for
/// the model (not those only views call).
fn enabled(mut snap: Snapshot, rules: &[Vec<String>]) -> Snapshot {
    snap.tools = snap
        .tools
        .iter()
        .filter(|t| any_rules_enable(rules, &t.name) && visible_to(t, "model"))
        .cloned()
        .collect();
    snap
}

/// Server `name`, when the session has it, with the tools it enables.
async fn bound(hub: &McpHub, mcp: &Servers, name: &str) -> ToolResult<(Arc<Server>, Snapshot)> {
    let Some(rules) = mcp.get(name) else {
        return Err(ToolError::model(format!(
            "no MCP server `{name}` in this session{}",
            if mcp.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = mcp.keys().map(String::as_str).collect();
                format!(" (it has: {})", names.join(", "))
            }
        )));
    };
    let server = hub
        .server(name)
        .ok_or_else(|| ToolError::model(format!("MCP server `{name}` is not configured")))?;
    let snap = enabled(server.ready().await, rules);
    if snap.status != McpStatus::Ready {
        return Err(ToolError::model(format!(
            "MCP server `{name}` is {}",
            status_line(&snap)
        )));
    }
    Ok((server, snap))
}

fn first_line(tool: &Tool) -> &str {
    tool.description
        .as_deref()
        .and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or("")
}

/// `read`'s and `search`'s view of one tool: an action of its server.
fn render(out: &mut String, server: &str, tool: &Tool, pretty: bool) {
    let _ = writeln!(out, "action {server}/{}", tool.name);
    if let Some(title) = &tool.title {
        let _ = writeln!(out, "{title}");
    }
    if let Some(d) = tool
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        let _ = writeln!(out, "{d}");
    }
    let schema = Value::Object((*tool.input_schema).clone());
    let schema = if pretty {
        serde_json::to_string_pretty(&schema)
    } else {
        serde_json::to_string(&schema)
    }
    .unwrap_or_default();
    let _ = writeln!(out, "Input schema: {schema}");
}

/// `list @rumu:/mcp`: the session's servers.
pub async fn list_servers(hub: &McpHub, mcp: &Servers) -> ToolOutput {
    if mcp.is_empty() {
        return "This session has no MCP servers.".into();
    }
    let mut out = String::new();
    for (name, rules) in mcp {
        let status = match hub.server(name) {
            Some(s) => status_line(&enabled(s.ready().await, rules)),
            None => "not configured".into(),
        };
        let _ = writeln!(out, "{name}/ — {status}");
    }
    out.into()
}

/// `list @rumu:/mcp/<server>`: its status, its resources, then its tools as
/// its actions (`<server>/<tool>(…)` and a line each) and after them
/// `actions`, the path's own.
pub async fn list_server(
    hub: &McpHub,
    mcp: &Servers,
    name: &str,
    actions: impl IntoIterator<Item = (String, String)>,
) -> ToolResult<ToolOutput> {
    let (server, snap) = bound(hub, mcp, name).await?;
    let lines = snap.tools.iter().map(|tool| {
        let schema = Value::Object((*tool.input_schema).clone());
        let sig = signature(&format!("{name}/{}", tool.name), &schema);
        (sig, first_line(tool).to_owned())
    });
    let mut out = format!(
        "MCP server `{name}`: {}; `read` it for its instructions and each tool's input schema. \
         Run a tool with `invoke` (no `path`).\n\n",
        status_line(&snap),
    );
    let resources = resources_section(&server).await;
    if !resources.is_empty() {
        let _ = write!(out, "{resources}\n\n");
    }
    out.push_str(&actions_section(lines.chain(actions)));
    Ok(out.into())
}

/// The `resources:` and `resource templates:` sections of a server's
/// listing; empty when it lists none.
async fn resources_section(server: &Arc<Server>) -> String {
    let listed = match server.resources().await {
        Ok(listed) => listed,
        Err(e) => return format!("resources: unavailable ({e})"),
    };
    let mut sections = Vec::new();
    let items = listed.resources.iter().map(|r| {
        let about = about(&r.name, r.title.as_deref(), r.description.as_deref());
        (r.uri.as_str(), about, r.mime_type.as_deref())
    });
    sections.push(section("resources:", items));
    let items = listed.templates.iter().map(|t| {
        let about = about(&t.name, t.title.as_deref(), t.description.as_deref());
        (t.uri_template.as_str(), about, t.mime_type.as_deref())
    });
    sections.push(section(
        "resource templates (fill in the URI to read):",
        items,
    ));
    sections.retain(|s| !s.is_empty());
    sections.join("\n\n")
}

/// A listing section of `uri — about (mime)` lines, the first
/// [`RESOURCES_SHOWN`] and a count of the rest; empty for no items.
fn section<'a>(
    title: &str,
    items: impl ExactSizeIterator<Item = (&'a str, String, Option<&'a str>)>,
) -> String {
    let total = items.len();
    if total == 0 {
        return String::new();
    }
    let mut out = title.to_owned();
    for (uri, about, mime) in items.take(RESOURCES_SHOWN) {
        let _ = write!(out, "\n  {uri} — {about}");
        if let Some(mime) = mime {
            let _ = write!(out, " ({mime})");
        }
    }
    if total > RESOURCES_SHOWN {
        let _ = write!(out, "\n  … {} more", total - RESOURCES_SHOWN);
    }
    out
}

/// A resource's line: its title (or name), and the first line of its
/// description.
fn about(name: &str, title: Option<&str>, description: Option<&str>) -> String {
    let mut about = title.unwrap_or(name).to_owned();
    if let Some(d) = description.and_then(|d| d.lines().map(str::trim).find(|l| !l.is_empty())) {
        let _ = write!(about, ": {d}");
    }
    about
}

/// `read @rumu:/mcp/<server>`: its instructions, then each tool with its
/// description and input schema, then its resources.
pub async fn read_server(hub: &McpHub, mcp: &Servers, name: &str) -> ToolResult<ToolOutput> {
    let (server, snap) = bound(hub, mcp, name).await?;
    let mut out = format!("MCP server `{name}`: {}\n", status_line(&snap));
    if let Some(i) = snap
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|i| !i.is_empty())
    {
        let _ = writeln!(out, "\n{i}");
    }
    for tool in snap.tools.iter() {
        out.push('\n');
        render(&mut out, name, tool, true);
    }
    let resources = resources_section(&server).await;
    if !resources.is_empty() {
        let _ = writeln!(out, "\n{resources}");
    }
    Ok(out.into())
}

/// `search` with a `@rumu:/mcp[/<server>]` scope: BM25 over the tools'
/// names, titles and descriptions; each hit prints like `read`.
pub async fn search(
    hub: &McpHub,
    mcp: &Servers,
    server: Option<&str>,
    query: &str,
    limit: usize,
) -> ToolResult<ToolOutput> {
    let servers: Vec<&str> = match server {
        None => mcp.keys().map(String::as_str).collect(),
        Some(name) => vec![name],
    };
    if query.trim().is_empty() {
        return Err(ToolError::model("search: empty query"));
    }
    let mut tools: Vec<(String, Tool)> = Vec::new();
    for name in servers {
        // A server that is down is skipped; scoping to it says why.
        match bound(hub, mcp, name).await {
            Ok((_, snap)) => tools.extend(snap.tools.iter().map(|t| (name.to_owned(), t.clone()))),
            Err(e) if server.is_some() => return Err(e),
            Err(_) => {}
        }
    }
    let docs: Vec<Doc> = tools
        .iter()
        .map(|(_, t)| {
            Doc::new(
                [
                    (t.name.as_ref(), 3.0),
                    (t.title.as_deref().unwrap_or(""), 2.0),
                    (t.description.as_deref().unwrap_or(""), 1.0),
                ],
                1.0,
            )
        })
        .collect();
    let hits = rank_docs(&docs, query, limit, false);
    if hits.is_empty() {
        return Ok(format!("No MCP tool matches `{query}`.").into());
    }
    let mut out = String::new();
    for (i, _) in hits {
        let (server, tool) = &tools[i];
        render(&mut out, server, tool, false);
        out.push('\n');
    }
    Ok(out.trim_end().to_owned().into())
}
