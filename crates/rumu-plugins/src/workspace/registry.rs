//! The configuration half of the `@rumu` workspace:
//!
//! - `@rumu:/workspaces` lists the registry; `@rumu:/workspaces/<name>`
//!   reads and writes one entry (`{root, ssh?, read_only?}`);
//! - `@rumu:/project/workspaces` is the session's project's ordered workspace
//!   list (the first is its default), read-before-write like a card.
//!
//! Creating a registry entry needs no approval; changing an existing one asks
//! the user (`workspace`), since other projects may mount it.

use rumu_cards::deck::Action;
use rumu_core::{
    Content, Cx, PathState, Structured, ToolError, ToolOutput, ToolResult, Utf8UnixPath,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::approve::{approve, deck_name};
use crate::workspaces::{entry_yaml, parse_entry, set_project_workspaces, upsert_workspace};

use super::WorkspacePlugin;
use super::tools::{replace_occurrences, slice_lines, unknown_rumu_path};

/// Payload of the read marker on `@rumu:/project/workspaces` (v1): the
/// content hash the last read returned. A write must be based on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProjectWorkspacesRead {
    pub hash: String,
}

impl Structured for ProjectWorkspacesRead {
    const TAG: &'static str = "project.workspaces.read";
    const V: u32 = 1;
}

/// What a `/workspaces…` or `/project…` path names.
enum ConfigPath<'a> {
    Workspaces,
    Entry(&'a str),
    Project,
    ProjectWorkspaces,
}

fn config_path(abs: &Utf8UnixPath) -> Option<ConfigPath<'_>> {
    let path = abs.as_str().trim_end_matches('/');
    match path {
        "/workspaces" => return Some(ConfigPath::Workspaces),
        "/project" => return Some(ConfigPath::Project),
        "/project/workspaces" => return Some(ConfigPath::ProjectWorkspaces),
        _ => {}
    }
    let name = path.strip_prefix("/workspaces/")?;
    (!name.is_empty() && !name.contains('/')).then_some(ConfigPath::Entry(name))
}

/// Whether a path belongs to this virtual subtree.
pub(super) fn is_config_path(abs: &Utf8UnixPath) -> bool {
    abs.as_str() == "/workspaces"
        || abs.starts_with("/workspaces/")
        || abs.as_str() == "/project"
        || abs.starts_with("/project/")
}

impl WorkspacePlugin {
    /// `list` under `/workspaces…` and `/project…`.
    pub(super) async fn config_list(&self, abs: &Utf8UnixPath, cx: &Cx) -> ToolResult<ToolOutput> {
        let state = cx.state()?;
        match config_path(abs) {
            Some(ConfigPath::Workspaces) => Ok(self.workspace_listing(&state).into()),
            Some(ConfigPath::Entry(name)) => self.entry_read(name, None, None),
            Some(ConfigPath::Project) => {
                self.project_names(&state)?;
                Ok("workspaces\n".into())
            }
            Some(ConfigPath::ProjectWorkspaces) => self.project_text(&state),
            None => Err(unknown_rumu_path(abs)),
        }
    }

    /// `read` under `/workspaces…` and `/project…`. Reading the project's
    /// list records the content hash a later write must be based on.
    pub(super) async fn config_read(
        &self,
        abs: &Utf8UnixPath,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let state = cx.state()?;
        match config_path(abs) {
            Some(ConfigPath::Workspaces) => Ok(self.workspace_listing(&state).into()),
            Some(ConfigPath::Entry(name)) => self.entry_read(name, offset, limit),
            Some(ConfigPath::Project) => {
                self.project_names(&state)?;
                Ok("workspaces\n".into())
            }
            Some(ConfigPath::ProjectWorkspaces) => {
                let out = self.project_text(&state)?;
                let marker = read_marker(&out.content.text);
                Ok(out.with_effect(marker))
            }
            None => Err(unknown_rumu_path(abs)),
        }
    }

    /// `write` under `/workspaces…` and `/project…`.
    pub(super) async fn config_write(
        &self,
        state: &PathState,
        abs: &Utf8UnixPath,
        content: &str,
        from: Option<&str>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if from.is_some() {
            return Err(ToolError::model(
                "`from` applies to cards, not configuration",
            ));
        }
        match config_path(abs) {
            Some(ConfigPath::Entry(name)) => self.entry_write(state, name, content, cx).await,
            Some(ConfigPath::ProjectWorkspaces) => self.project_write(state, content, cx).await,
            Some(ConfigPath::Workspaces | ConfigPath::Project) => Err(ToolError::model(format!(
                "`@rumu:{abs}` is a directory; write a file under it"
            ))),
            None => Err(unknown_rumu_path(abs)),
        }
    }

    /// `edit` under `/workspaces…` and `/project…`: replace on the current
    /// text, then take the write path.
    pub(super) async fn config_edit(
        &self,
        state: &PathState,
        abs: &Utf8UnixPath,
        old: &str,
        new: &str,
        all: bool,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let (text, project) = match config_path(abs) {
            Some(ConfigPath::Entry(name)) => {
                let Some(ws) = self.workspaces.get(name) else {
                    return Err(no_workspace(name));
                };
                (
                    entry_yaml(&ws).map_err(|e| ToolError::model(e.to_string()))?,
                    false,
                )
            }
            Some(ConfigPath::ProjectWorkspaces) => (self.project_text(state)?.content.text, true),
            Some(ConfigPath::Workspaces | ConfigPath::Project) => {
                return Err(ToolError::model(format!(
                    "`@rumu:{abs}` is a directory; edit a file under it"
                )));
            }
            None => return Err(unknown_rumu_path(abs)),
        };
        let (new_text, n) = replace_occurrences(&text, old, new, all)
            .map_err(|e| ToolError::model(e.to_string()))?;
        if project {
            // Edit applies to the current text; it is not the stale-write
            // check `write` has.
            let after = self.project_apply(state, &new_text)?;
            return Ok(ToolOutput::ok(format!(
                "edited `@rumu:{abs}` ({n} replacement{}):\n{after}",
                if n == 1 { "" } else { "s" }
            ))
            .with_effect(read_marker(&after)));
        }
        let mut out = self.config_write(state, abs, &new_text, None, cx).await?;
        if !out.is_error {
            out.content.text = format!(
                "edited `@rumu:{abs}` ({n} replacement{})",
                if n == 1 { "" } else { "s" }
            );
        }
        Ok(out)
    }

    /// One line per registered workspace: the default `*`, name, machine
    /// label, root, rw/ro, and `(mounted)` for the project's.
    fn workspace_listing(&self, state: &PathState) -> String {
        let list = self.workspaces.list();
        if list.is_empty() {
            return "(no workspaces)".to_owned();
        }
        list.iter()
            .map(|ws| {
                let star = if state.default_workspace.as_deref() == Some(ws.name.as_str()) {
                    "* "
                } else {
                    "  "
                };
                let rw = if ws.read_only { "ro" } else { "rw" };
                let mut line = format!("{star}{} {} {} {rw}", ws.name, ws.label(), ws.root);
                if state.mounted.iter().any(|n| n == &ws.name) {
                    line.push_str("  (mounted)");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The YAML of one registry entry.
    fn entry_read(
        &self,
        name: &str,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> ToolResult<ToolOutput> {
        let Some(ws) = self.workspaces.get(name) else {
            return Err(no_workspace(name));
        };
        let text = entry_yaml(&ws).map_err(|e| ToolError::model(e.to_string()))?;
        let text = slice_lines(text, offset, limit).map_err(|e| ToolError::model(e.to_string()))?;
        Ok(text.into())
    }

    /// The session's project and its mounted workspaces, in order.
    fn project_names(&self, state: &PathState) -> ToolResult<(String, Vec<String>)> {
        let Some(project) = state.project.clone() else {
            return Err(ToolError::model(
                "this session is in no project; `@rumu:/project/workspaces` exists only in one \
                 (`card_mount(deck, at: \"project\")`)",
            ));
        };
        let names = self.workspaces.project(&project);
        Ok((project, names))
    }

    /// The project's list as YAML (`- name` lines); empty when none.
    fn project_text(&self, state: &PathState) -> ToolResult<ToolOutput> {
        let (_, names) = self.project_names(state)?;
        Ok(project_yaml(&names).into())
    }

    /// Parse `content` as the project's new list and save it.
    fn project_apply(&self, state: &PathState, content: &str) -> ToolResult<String> {
        let (project, _) = self.project_names(state)?;
        let names: Option<Vec<String>> = rumu_cards::yaml::from_str(content)
            .map_err(|e| ToolError::model(format!("workspaces list does not parse: {e}")))?;
        let names = names.unwrap_or_default();
        set_project_workspaces(&self.home, &self.workspaces, &project, names.clone())
            .map_err(|e| ToolError::model(e.to_string()))?;
        Ok(project_yaml(&names))
    }

    /// Create or change one registry entry. A change asks the user; identical
    /// content is a no-op.
    async fn entry_write(
        &self,
        state: &PathState,
        name: &str,
        content: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let ws = parse_entry(name, content).map_err(|e| ToolError::model(e.to_string()))?;
        let after = entry_yaml(&ws).map_err(|e| ToolError::model(e.to_string()))?;
        if let Some(current) = self.workspaces.get(name) {
            let before = entry_yaml(&current).map_err(|e| ToolError::model(e.to_string()))?;
            if before == after {
                return Ok(ToolOutput::ok(format!("workspace `{name}` is unchanged")));
            }
            let request = json!({
                "kind": "workspace_change",
                "from": deck_name(self.decks(), &state.assistant),
                "from_id": state.assistant,
                "name": name,
                "before": before,
                "after": after,
            });
            if !approve(self.decks(), cx, Action::Workspace, name, request)
                .await
                .map_err(|e| ToolError::model(format!("workspace: {e}")))?
            {
                return Err(ToolError::model(format!(
                    "the user declined to change workspace `{name}`"
                )));
            }
        }
        upsert_workspace(&self.home, &self.workspaces, ws)
            .map_err(|e| ToolError::model(e.to_string()))?;
        Ok(ToolOutput::ok(format!("wrote `@rumu:/workspaces/{name}`")))
    }

    /// Replace the project's list, after the read-before-write check.
    async fn project_write(
        &self,
        state: &PathState,
        content: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let current = self.project_text(state)?.content.text;
        let hash = rumu_cards::card::view_rev_of(&current);
        let seen = cx.index()?.latest::<ProjectWorkspacesRead>();
        if seen.map(|e| e.value.hash) != Some(hash) {
            return Err(ToolError::model(format!(
                "`@rumu:/project/workspaces` changed since you read it; read it again first. \
                 Current list:\n{current}"
            )));
        }
        let after = self.project_apply(state, content)?;
        Ok(
            ToolOutput::ok(format!("wrote `@rumu:/project/workspaces`:\n{after}"))
                .with_effect(read_marker(&after)),
        )
    }
}

/// The read marker for the list `text`: the base of the next write (a write
/// of one's own is as good as a read).
fn read_marker(text: &str) -> Content {
    Content::marker(&ProjectWorkspacesRead {
        hash: rumu_cards::card::view_rev_of(text),
    })
}

fn project_yaml(names: &[String]) -> String {
    serde_saphyr::to_string(&names.to_vec()).unwrap_or_else(|_| "[]\n".to_owned())
}

fn no_workspace(name: &str) -> ToolError {
    ToolError::model(format!(
        "no workspace `{name}`; `list @rumu:/workspaces` shows every registered one"
    ))
}
