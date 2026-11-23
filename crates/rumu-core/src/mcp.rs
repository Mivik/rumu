//! MCP server settings and what the web sees of them (see `docs/mcp.md`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One server under `mcp` in `settings.yaml`: either `command` (stdio) or
/// `url` (streamable HTTP).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct McpServerSettings {
    /// The program and its arguments, run in `RUMU_HOME`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// Added to rumu's environment. Values never leave the server.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Sent with every request. Values never leave the server.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// Whether `name` can name an MCP server: 1–64 of `[A-Za-z0-9_-]`.
pub fn valid_mcp_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpStatus {
    Connecting,
    Ready,
    Failed,
}

/// A server as the web sees it: `env` and `headers` by name only.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct McpServerInfo {
    pub name: String,
    pub command: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
    pub env: Vec<String>,
    pub headers: Vec<String>,
    pub status: McpStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    pub tools: Vec<McpToolInfo>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct McpToolInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
}

/// Payload of an MCP tool result shown as an MCP App view (`mcp.app` v1):
/// the tool named a `ui://` resource, read along with the call. Both the
/// page and the call are blobs, so the view replays as it first showed
/// whatever later happens to the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct McpApp {
    pub server: String,
    pub tool: String,
    /// The `ui://` resource.
    pub uri: String,
    /// Blob of the view's HTML.
    pub html: String,
    /// Blob of `{"arguments": …, "result": …}`: the call's arguments and
    /// its `CallToolResult` as the server sent it.
    pub call: String,
    /// The origins the view may reach, from the resource's `_meta.ui.csp`.
    #[serde(default)]
    pub csp: McpAppCsp,
    /// The resource's `_meta.ui.prefersBorder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prefers_border: Option<bool>,
}

impl crate::Structured for McpApp {
    const TAG: &'static str = "mcp.app";
    const V: u32 = 1;
}

/// The origins an MCP App view may reach (`McpUiResourceCsp`), each a CSP
/// source expression; none is the restrictive default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default)]
#[ts(export)]
pub struct McpAppCsp {
    /// `connect-src`.
    pub connect: Vec<String>,
    /// `script-src`, `style-src`, `img-src`, `font-src`, `media-src`.
    pub resource: Vec<String>,
    /// `frame-src`.
    pub frame: Vec<String>,
    /// `base-uri`.
    pub base_uri: Vec<String>,
}

/// Payload of the note an MCP App view's `ui/update-model-context` became
/// (`mcp.app_context` v1): the latest the view said before a model call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct McpAppContext {
    pub server: String,
    pub tool: String,
}

impl crate::Structured for McpAppContext {
    const TAG: &'static str = "mcp.app_context";
    const V: u32 = 1;
}

/// An assistant's MCP servers: each server's name in `settings.yaml`, with
/// the rules enabling its tools ([`crate::rules_enable`]; `*` is all of
/// them, new ones included).
pub type McpUse = BTreeMap<String, Vec<String>>;
