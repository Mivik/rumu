//! The configuration half of the `@rumu` workspace:
//!
//! - `@rumu:/workspaces` lists the workspaces (the project's first, then the
//!   rest of the registry, then `@scratch` and `@rumu`);
//!   `@rumu:/workspaces/<name>` reads and writes one registry entry
//!   (`{root, ssh?, read_only?}`), and its `workspace.use` makes it the
//!   branch's default (`@scratch` and `@rumu` have entries for that alone);
//! - `@rumu:/project/workspaces` is the session's project's ordered workspace
//!   list (the first is its default), read-before-write like a card.
//!
//! Creating a registry entry needs no approval; changing or deleting an
//! existing one asks the user (`workspace`), since other projects may mount
//! it.

use async_trait::async_trait;
use rumu_cards::deck::Action;
use rumu_core::{
    Content, Cx, PathState, RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, ToolError, ToolOutput,
    ToolResult, Utf8UnixPath,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::approve::{approve, deck_name};
use crate::workspaces::{
    entry_yaml, parse_entry, remove_workspace, set_project_workspaces, upsert_workspace,
};

use super::WorkspacePlugin;
use super::tools::replace_occurrences;
use super::vfs::{ActionSpec, Approval, At, NoArgs, Node, Out};

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

/// `/workspaces`: one line per workspace: the default `*`, name, machine
/// label, root, rw/ro, and `(mounted)` for the project's.
pub(super) struct WorkspacesDir;

#[async_trait]
impl Node for WorkspacesDir {
    async fn list(&self, at: &At<'_>) -> Out {
        Ok(at.p.listing(at.state).await.into())
    }

    fn holds_files(&self) -> bool {
        true
    }
}

/// `/workspaces/<name>`: one registry entry, or `@scratch` / `@rumu`.
pub(super) struct Entry;

#[async_trait]
impl Node for Entry {
    async fn list(&self, at: &At<'_>) -> Out {
        let name = at.param("name");
        match name {
            RUMU_WORKSPACE => Ok(
                "# rumu's own data (cards, history, sessions, tasks, …); not in \
                 the registry; no bash\n"
                    .into(),
            ),
            SCRATCH_WORKSPACE => {
                let root = at.state.workspaces.iter().find(|w| w.name == name);
                let root = root.map(|w| w.root.to_string()).unwrap_or_default();
                Ok(format!(
                    "# this session's throwaway directory, removed with it; not in the registry\n\
                     root: {root}\n"
                )
                .into())
            }
            _ => Ok(at.p.entry_text(name)?.into()),
        }
    }

    async fn write(&self, at: &At<'_>, content: &str, from: Option<&str>) -> Out {
        no_from(from)?;
        let name = fixed(at.param("name"))?;
        at.p.entry_write(at.state, name, content, at.cx).await
    }

    /// Replace on the current text, then take the write path.
    async fn edit(&self, at: &At<'_>, old: &str, new: &str, all: bool) -> Out {
        let name = fixed(at.param("name"))?;
        let text = at.p.entry_text(name)?;
        let (new_text, n) = replace_occurrences(&text, old, new, all)
            .map_err(|e| ToolError::model(e.to_string()))?;
        let mut out = at.p.entry_write(at.state, name, &new_text, at.cx).await?;
        if !out.is_error {
            out.content.text = edited(at.abs, n);
        }
        Ok(out)
    }

    async fn delete(&self, at: &At<'_>) -> Out {
        at.p.entry_delete(at.state, fixed(at.param("name"))?, at.cx)
            .await
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[ActionSpec {
            name: "workspace.use",
            about: "make it this branch's default workspace (forks inherit it)",
            args: rumu_core::tool_schema::<NoArgs>,
            approval: Approval::Never,
        }]
    }

    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        let NoArgs {} = spec.parse(args)?;
        at.p.use_workspace(at.state, at.param("name"), at.cx).await
    }
}

/// `name`, unless it is `@scratch` or `@rumu`: they are listed beside the
/// registry, not in it, and only to be used.
fn fixed(name: &str) -> ToolResult<&str> {
    if name == RUMU_WORKSPACE || name == SCRATCH_WORKSPACE {
        return Err(ToolError::model(format!(
            "`{name}` is not in the registry and cannot be changed; `workspace.use` is all it has"
        )));
    }
    Ok(name)
}

/// `/project`: the session's project (only `workspaces` under it).
pub(super) struct ProjectDir;

#[async_trait]
impl Node for ProjectDir {
    async fn list(&self, at: &At<'_>) -> Out {
        at.p.project_names(at.state)?;
        Ok("workspaces\n".into())
    }

    fn holds_files(&self) -> bool {
        true
    }
}

/// `/project/workspaces`: the project's ordered workspace list. Reading it
/// records the content hash a later write must be based on.
pub(super) struct ProjectWorkspaces;

#[async_trait]
impl Node for ProjectWorkspaces {
    async fn list(&self, at: &At<'_>) -> Out {
        at.p.project_text(at.state)
    }

    async fn read(&self, at: &At<'_>, _: Option<u64>, _: Option<u64>) -> Out {
        let out = at.p.project_text(at.state)?;
        let marker = read_marker(&out.content.text);
        Ok(out.with_effect(marker))
    }

    async fn write(&self, at: &At<'_>, content: &str, from: Option<&str>) -> Out {
        no_from(from)?;
        at.p.project_write(at.state, content, at.cx).await
    }

    /// Edit applies to the current text; it is not the stale-write check
    /// `write` has.
    async fn edit(&self, at: &At<'_>, old: &str, new: &str, all: bool) -> Out {
        let text = at.p.project_text(at.state)?.content.text;
        let (new_text, n) = replace_occurrences(&text, old, new, all)
            .map_err(|e| ToolError::model(e.to_string()))?;
        let after = at.p.project_apply(at.state, &new_text)?;
        Ok(ToolOutput::ok(format!("{}:\n{after}", edited(at.abs, n)))
            .with_effect(read_marker(&after)))
    }
}

/// Configuration is written whole: `from` (a move) is for cards.
fn no_from(from: Option<&str>) -> ToolResult<()> {
    match from {
        Some(_) => Err(ToolError::model(
            "`from` applies to cards, not configuration",
        )),
        None => Ok(()),
    }
}

/// The result line of an edit.
fn edited(abs: &Utf8UnixPath, n: usize) -> String {
    format!(
        "edited `@rumu:{abs}` ({n} replacement{})",
        if n == 1 { "" } else { "s" }
    )
}

impl WorkspacePlugin {
    /// The YAML of one registry entry.
    fn entry_text(&self, name: &str) -> ToolResult<String> {
        let Some(ws) = self.workspaces.get(name) else {
            return Err(no_workspace(name));
        };
        entry_yaml(&ws).map_err(|e| ToolError::model(e.to_string()))
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

    /// Remove one registry entry, and from every project mounting it (what
    /// the web's `workspace.delete` does), with the user's approval.
    async fn entry_delete(&self, state: &PathState, name: &str, cx: &Cx) -> Out {
        let Some(current) = self.workspaces.get(name) else {
            return Err(no_workspace(name));
        };
        let before = entry_yaml(&current).map_err(|e| ToolError::model(e.to_string()))?;
        let request = json!({
            "kind": "workspace_delete",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "name": name,
            "before": before,
        });
        if !approve(self.decks(), cx, Action::Workspace, name, request)
            .await
            .map_err(|e| ToolError::model(format!("workspace: {e}")))?
        {
            return Err(ToolError::model(format!(
                "the user declined to delete workspace `{name}`"
            )));
        }
        remove_workspace(&self.home, &self.workspaces, name)
            .map_err(|e| ToolError::model(e.to_string()))?;
        Ok(ToolOutput::ok(format!(
            "deleted `@rumu:/workspaces/{name}`; no project mounts it any more"
        )))
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
