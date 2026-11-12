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
