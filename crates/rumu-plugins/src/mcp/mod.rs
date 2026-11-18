//! MCP: servers registered in `settings.yaml` (see [`McpHub`]), bound to
//! assistants by name. Their tools stay out of the model's tool list: the
//! model finds them under `@rumu:/tools` (`list`, `read`, `search`, served
//! by the workspace plugin through the functions here) and calls them with
//! `call_tool`. See `docs/mcp.md`.

mod hub;

#[cfg(test)]
mod tests;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use base64::Engine as _;
use rmcp::model::{JsonObject, Tool};
use rumu_cards::Decks;
use rumu_cards::recall::{Doc, rank_docs};
use rumu_core::{
    Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, McpStatus, McpUse, PathState,
    Plugin, RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, ToolError, ToolOutput, ToolResult,
    ToolResultExt, ToolSpec, parse_args, rules_enable, tool_schema,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use hub::{McpHub, Server, Snapshot};

/// A server's `instructions` in the first-call note are cut to this many
/// characters.
const INSTRUCTIONS_LIMIT: usize = 2_000;
/// Default `search` hits under `@rumu:/tools`: each prints its schema.
pub const SEARCH_LIMIT: usize = 5;
const INTRO_HOOK: &str = "mcp-intro";

/// Payload of the `mcp.intro` note (v1): the server list, again whenever it
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct McpIntro {
    servers: Vec<String>,
}

impl Structured for McpIntro {
    const TAG: &'static str = "mcp.intro";
    const V: u32 = 1;
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CallToolArgs {
    /// `<server>/<tool>`, as listed under `@rumu:/tools`
    tool: String,
    /// the tool's arguments, per its input schema
    #[serde(default)]
    args: Option<JsonObject>,
}

/// The `call_tool` tool and the server-list note.
pub struct McpPlugin {
    hub: Arc<McpHub>,
    decks: Decks,
    home: PathBuf,
    blobs: OnceLock<rumu_blob::Blobs>,
}

impl McpPlugin {
    pub fn new(home: &std::path::Path, hub: Arc<McpHub>, decks: Decks) -> Self {
        Self {
            hub,
            decks,
            home: home.to_owned(),
            blobs: OnceLock::new(),
        }
    }

    fn blobs(&self) -> rumu_blob::Result<rumu_blob::Blobs> {
        if let Some(b) = self.blobs.get() {
            return Ok(b.clone());
        }
        let opened = rumu_blob::Blobs::in_home(&self.home)?;
        Ok(self.blobs.get_or_init(|| opened).clone())
    }
}

#[async_trait]
impl Plugin for McpPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "mcp".into(),
            instructions: None,
            tools: vec![ToolSpec {
                name: "call_tool".into(),
                description: format!(
                    "Call a tool of an MCP server bound to this session. These tools are not in \
your tool list: find them with `search` (scope `{RUMU_WORKSPACE}:/tools`, whose hits show each \
input schema) or `list {RUMU_WORKSPACE}:/tools/<server>`, and read one's input schema with \
`read {RUMU_WORKSPACE}:/tools/<server>/<tool>` before calling it."
                ),
                schema: tool_schema::<CallToolArgs>(),
                background: true,
                output_schema: None,
                parallel: false,
            }],
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
        let text = match (last, servers.is_empty()) {
            (_, true) => "This session has no MCP servers any more.".to_owned(),
            (None, false) => intro(&self.hub, &mcp).await,
            (Some(_), false) => format!(
                "The MCP servers changed; this replaces the earlier list.\n{}",
                intro(&self.hub, &mcp).await
            ),
        };
        Ok(HookOutput {
            effects: vec![Content::note_with(text, &McpIntro { servers })],
            decision: Decision::Pass,
        })
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        match name {
            "call_tool" => self.call(&args, cx).await.finish(),
            other => Ok(ToolOutput::err(format!(
                "plugin `mcp` has no tool `{other}`"
            ))),
        }
    }
}

impl McpPlugin {
    async fn call(&self, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: CallToolArgs = parse_args("call_tool", args)?;
        let state = cx.state()?;
        let mcp = servers(&self.decks, &state);
        let (name, tool) = args
            .tool
            .split_once('/')
            .ok_or_else(|| ToolError::model("call_tool: `tool` is `<server>/<tool>`"))?;
        let (server, snap) = bound(&self.hub, &mcp, name).await?;
        if !snap.tools.iter().any(|t| t.name == tool) {
            return Err(ToolError::model(format!(
                "call_tool: `{name}` has no tool `{tool}`; its tools: {}",
                names(&snap.tools)
            )));
        }
        let result = server
            .call(tool, args.args.unwrap_or_default())
            .await
            .map_err(|e| ToolError::model(format!("call_tool {}: {e}", args.tool)))?;
        let scratch = state
            .workspaces
            .iter()
            .find(|w| w.name == SCRATCH_WORKSPACE)
            .map(|w| PathBuf::from(w.root.as_str()));
        self.output(&result, scratch).await
    }

    /// A `tools/call` result as a tool output: text blocks joined, images
    /// attached — and copied into `scratch` (the session's `@scratch` root)
    /// like a prompt's, the text naming the file — other media named.
    async fn output(&self, result: &Value, scratch: Option<PathBuf>) -> ToolResult<ToolOutput> {
        let mut parts: Vec<String> = Vec::new();
        let mut images = Vec::new();
        for block in result["content"].as_array().into_iter().flatten() {
            match block["type"].as_str().unwrap_or("") {
                "text" => parts.push(block["text"].as_str().unwrap_or("").to_owned()),
                "image" => {
                    let data = block["data"].as_str().unwrap_or("");
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map_err(|e| ToolError::model(format!("call_tool: bad image data: {e}")))?;
                    let blobs = self.blobs().map_err(|e| {
                        ToolError::model(format!("call_tool: storing an image: {e}"))
                    })?;
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
                        .map_err(|e| ToolError::model(format!("call_tool: {e}")))?
                        .map_err(|e| ToolError::model(format!("call_tool: an image: {e}")))?;
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
/// instructions.
async fn intro(hub: &McpHub, mcp: &McpUse) -> String {
    let mut text = format!(
        "MCP servers of this session. Their tools are not in your tool list: find them with \
`search` (scope `{RUMU_WORKSPACE}:/tools`) or `list {RUMU_WORKSPACE}:/tools/<server>`, and call \
them with `call_tool`."
    );
    for (name, rules) in mcp {
        let _ = write!(text, "\n- `{name}`: ");
        match hub.server(name) {
            None => text.push_str("not configured"),
            Some(server) => {
                let snap = enabled(server.ready().await, rules);
                text.push_str(&status_line(&snap));
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
    text
}

fn status_line(snap: &Snapshot) -> String {
    match snap.status {
        McpStatus::Ready => format!("{} tools", snap.tools.len()),
        McpStatus::Connecting => "connecting".into(),
        McpStatus::Failed => format!(
            "unavailable ({})",
            snap.error
                .as_deref()
                .and_then(|e| e.lines().next())
                .unwrap_or("failed")
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

/// `read`'s and `search`'s view of one tool.
fn render(out: &mut String, server: &str, tool: &Tool, pretty: bool) {
    let _ = writeln!(out, "## {server}/{}", tool.name);
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

/// `list @rumu:/tools`: the session's servers.
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

/// `list @rumu:/tools/<server>`: one server's tools.
pub async fn list_tools(hub: &McpHub, mcp: &McpUse, name: &str) -> ToolResult<ToolOutput> {
    let (_, snap) = bound(hub, mcp, name).await?;
    let mut out = String::new();
    for tool in snap.tools.iter() {
        let _ = writeln!(out, "{} — {}", tool.name, first_line(tool));
    }
    Ok(out.into())
}

/// `read @rumu:/tools/<server>/<tool>`: one tool's description and input
/// schema.
pub async fn read_tool(
    hub: &McpHub,
    mcp: &McpUse,
    name: &str,
    tool: &str,
) -> ToolResult<ToolOutput> {
    let (_, snap) = bound(hub, mcp, name).await?;
    let tool = snap.tools.iter().find(|t| t.name == tool).ok_or_else(|| {
        ToolError::model(format!(
            "`{name}` has no tool `{tool}`; its tools: {}",
            names(&snap.tools)
        ))
    })?;
    let mut out = String::new();
    render(&mut out, name, tool, true);
    Ok(out.into())
}

/// `search` with a `@rumu:/tools[/<server>]` scope: BM25 over the tools'
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
