//! MCP: servers registered in `settings.yaml` (see [`McpHub`]), bound to
//! assistants by name. Their tools stay out of the model's tool list: each
//! server is an object under `@rumu:/mcp` whose actions are its tools
//! (`<server>/<tool>`). The model finds them with `list`, `read` and
//! `search` there and runs one with `invoke`, all served by the workspace
//! plugin through the functions here. See `docs/mcp.md`.

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
    Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, McpStatus, McpUse, PathState,
    Plugin, RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, ToolError, ToolOutput, ToolResult,
    rules_enable,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::workspace::vfs::{actions_section, signature};

pub use hub::{McpHub, Server, Snapshot};

/// A server's `instructions` in the first-call note are cut to this many
/// characters.
const INSTRUCTIONS_LIMIT: usize = 2_000;
/// Default `search` hits under `@rumu:/mcp`: each prints its schema.
pub const SEARCH_LIMIT: usize = 5;
const INTRO_HOOK: &str = "mcp-intro";

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
        let state = cx.state()?;
        let mcp = servers(&self.decks, &state);
        let servers: Vec<String> = mcp.keys().cloned().collect();
        let last = cx.index()?.all::<McpIntro>().pop().map(|e| e.value.servers);
        if last.as_ref().unwrap_or(&Vec::new()) == &servers {
            return Ok(HookOutput::default());
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
        Ok(HookOutput {
            effects: vec![Content::note_with(text, &McpIntro { servers, status })],
            decision: Decision::Pass,
        })
    }
}

/// `invoke` of `<name>/<tool>`: run the tool of one of the session's
/// servers (`mcp`) and turn its result into a tool output. A call the server
/// refuses comes back with the tool's input schema.
pub async fn call(
    hub: &McpHub,
    mcp: &McpUse,
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
    let result = server.call(tool, args).await.map_err(|e| {
        let schema = Value::Object((*spec.input_schema).clone());
        ToolError::model(format!("{name}/{tool}: {e}; its arguments: {schema}"))
    })?;
    let scratch = state
        .workspaces
        .iter()
        .find(|w| w.name == SCRATCH_WORKSPACE)
        .map(|w| PathBuf::from(w.root.as_str()));
    output(&result, scratch, blobs).await
}

/// A `tools/call` result as a tool output: text blocks joined, images
/// attached — and copied into `scratch` (the session's `@scratch` root)
/// like a prompt's, the text naming the file — other media named.
async fn output(
    result: &Value,
    scratch: Option<PathBuf>,
    blobs: impl Fn() -> rumu_blob::Result<rumu_blob::Blobs>,
) -> ToolResult<ToolOutput> {
    let mut parts: Vec<String> = Vec::new();
    let mut images = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        match block["type"].as_str().unwrap_or("") {
            "text" => parts.push(block["text"].as_str().unwrap_or("").to_owned()),
            "image" => {
                let data = block["data"].as_str().unwrap_or("");
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| ToolError::model(format!("bad image data: {e}")))?;
                let blobs =
                    blobs().map_err(|e| ToolError::model(format!("storing an image: {e}")))?;
                let root = scratch.clone();
                let (attachment, file) =
                    tokio::task::spawn_blocking(move || -> rumu_blob::Result<_> {
                        let a = blobs.ingest_image(&bytes, None)?;
                        let file = a.scratch_path();
                        if let Some(root) = root {
                            blobs.copy_to(&a.hash, &root.join(&file))?;
                        }
                        Ok((a, file))
                    })
                    .await
                    .map_err(|e| ToolError::model(e.to_string()))?
                    .map_err(|e| ToolError::model(format!("an image: {e}")))?;
                images.push(attachment);
                if scratch.is_some() {
                    parts.push(format!(
                        "[image {} saved as {SCRATCH_WORKSPACE}:{file}]",
                        images.len()
                    ));
                }
            }
            "audio" => parts.push(format!(
                "[audio ({}) omitted]",
                block["mimeType"].as_str().unwrap_or("?")
            )),
            "resource" => {
                let r = &block["resource"];
                match r["text"].as_str() {
                    Some(text) => parts.push(text.to_owned()),
                    None => parts.push(format!(
                        "[resource {} ({}) omitted]",
                        r["uri"].as_str().unwrap_or("?"),
                        r["mimeType"].as_str().unwrap_or("binary")
                    )),
                }
            }
            "resource_link" => parts.push(format!(
                "[resource link: {}{}]",
                block["uri"].as_str().unwrap_or("?"),
                block["name"]
                    .as_str()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default()
            )),
            other => parts.push(format!("[{other} content omitted]")),
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

/// The servers of the session's assistant and the rules enabling their tools,
/// from its deck as it is now: a change to the assistant reaches its running
/// sessions.
pub fn servers(decks: &Decks, state: &PathState) -> McpUse {
    decks
        .get(&state.assistant)
        .map(|d| d.meta.mcp)
        .unwrap_or_default()
}

/// The server-list note: the session's servers, their status and
/// instructions; and each configured server's state for the UI.
async fn intro(hub: &McpHub, mcp: &McpUse) -> (String, BTreeMap<String, McpIntroServer>) {
    let mut status = BTreeMap::new();
    let mut text = format!(
        "MCP servers of this session. Their tools are not in your tool list: they are the actions \
of `{RUMU_WORKSPACE}:/mcp/<server>` — find them with `search` (scope `{RUMU_WORKSPACE}:/mcp`) or \
`list {RUMU_WORKSPACE}:/mcp/<server>`, and run one with `invoke` (`action: \"<server>/<tool>\"`, \
no `path`)."
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

/// `snap` with only the tools `rules` enable.
fn enabled(mut snap: Snapshot, rules: &[String]) -> Snapshot {
    snap.tools = snap
        .tools
        .iter()
        .filter(|t| rules_enable(rules, &t.name))
        .cloned()
        .collect();
    snap
}

/// Server `name`, when the session has it, with the tools it enables.
async fn bound(hub: &McpHub, mcp: &McpUse, name: &str) -> ToolResult<(Arc<Server>, Snapshot)> {
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
pub async fn list_servers(hub: &McpHub, mcp: &McpUse) -> ToolOutput {
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

/// `list @rumu:/mcp/<server>`: its status, then its tools as its actions
/// (`<server>/<tool>(…)` and a line each).
pub async fn list_server(hub: &McpHub, mcp: &McpUse, name: &str) -> ToolResult<ToolOutput> {
    let (_, snap) = bound(hub, mcp, name).await?;
    let lines = snap.tools.iter().map(|tool| {
        let schema = Value::Object((*tool.input_schema).clone());
        let sig = signature(&format!("{name}/{}", tool.name), &schema);
        (sig, first_line(tool).to_owned())
    });
    Ok(format!(
        "MCP server `{name}`: {}; `read` it for its instructions and each tool's input schema. \
         Run a tool with `invoke` (no `path`).\n\n{}",
        status_line(&snap),
        actions_section(lines)
    )
    .into())
}

/// `read @rumu:/mcp/<server>`: its instructions, then each tool with its
/// description and input schema.
pub async fn read_server(hub: &McpHub, mcp: &McpUse, name: &str) -> ToolResult<ToolOutput> {
    let (_, snap) = bound(hub, mcp, name).await?;
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
    Ok(out.into())
}

/// `search` with a `@rumu:/mcp[/<server>]` scope: BM25 over the tools'
/// names, titles and descriptions; each hit prints like `read`.
pub async fn search(
    hub: &McpHub,
    mcp: &McpUse,
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
