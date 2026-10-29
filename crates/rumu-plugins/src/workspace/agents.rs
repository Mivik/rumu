//! AGENTS.md / CLAUDE.md injection: when a workspace is used on a branch, its
//! instructions file becomes a note. The branch's index records which file
//! and content hash were injected, so a changed file is re-injected once with
//! an "(updated)" header.

use rumu_core::{Content, Cx, Effect, Structured, Utf8TypedPathBuf, Workspace, normalize_abs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::WorkspacePlugin;
use super::backend;

/// Payload of the `workspace.agents` note (v1): which file and content hash
/// this branch already injected for the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct AgentsNote {
    pub ws: String,
    pub file: String,
    pub hash: String,
}

impl Structured for AgentsNote {
    const TAG: &'static str = "workspace.agents";
    const V: u32 = 1;
}

impl WorkspacePlugin {
    /// When a workspace is used (a tool call targeting it, or `workspace_use`),
    /// inject its AGENTS.md (or CLAUDE.md) as a note: once per path, and again
    /// only when the file's content hash differs from the latest injection.
    /// An empty file is skipped entirely. Reads go through the workspace
    /// backend. Ssh workspaces confirmed to have neither file are remembered
    /// (the probe is not free) and not re-probed; local ones are re-checked,
    /// and a failed ssh probe (e.g. host down) is not cached.
    pub(super) async fn maybe_inject_agents(&self, cx: &Cx, ws: &Workspace) -> Vec<Effect> {
        let miss_key = format!("{}:{}", ws.label(), ws.root);
        if self.agents_missing.lock().unwrap().contains(&miss_key) {
            return vec![];
        }
        let backend = self.backend_for(ws);
        for candidate in ["AGENTS.md", "CLAUDE.md"] {
            let abs = normalize_abs(ws, candidate)
                .unwrap_or_else(|_| Utf8TypedPathBuf::from_unix(format!("/{candidate}")));
            let read = backend.read_bytes(&abs).await;
            if let Err(e) = &read
                && !backend::is_not_found(e)
            {
                return vec![]; // transient (ssh down, permissions): retry next time
            }
            let Ok(bytes) = read else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if text.trim().is_empty() {
                return vec![];
            }
            let hash = sha256_prefix(&text);
            let latest = cx.index().ok().and_then(|index| {
                index
                    .all::<AgentsNote>()
                    .into_iter()
                    .rfind(|e| e.value.ws == ws.name)
            });
            if latest.as_ref().is_some_and(|e| e.value.hash == hash) {
                return vec![];
            }
            let marker = if latest.is_some() { " (updated)" } else { "" };
            let body = format!("# {candidate} (workspace `{}`){marker}\n\n{text}", ws.name);
            return vec![Content::note_with(
                body,
                &AgentsNote {
                    ws: ws.name.clone(),
                    file: candidate.to_owned(),
                    hash,
                },
            )];
        }
        if !ws.is_local() {
            self.agents_missing.lock().unwrap().insert(miss_key);
        }
        vec![]
    }
}

/// The first 16 hex chars of the SHA-256 of `text`.
fn sha256_prefix(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}
