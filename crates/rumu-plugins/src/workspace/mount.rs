//! The branch's card mount table, changed by the `deck.mount` and
//! `deck.unmount` actions of `@rumu:/decks/<name>`.
//!
//! Mounts are branch state (a note marker on the path), so forks and
//! sub-agents inherit them. Mounting makes a deck readable; writes to it are
//! approved separately (see [`crate::approve`]). The mount result carries the
//! mounted deck's pinned cards and catalog; the unmount result says the deck
//! is no longer readable/writable.
//!
//! A session is in one project at most: the mount marked
//! [`CardMount::project`]. Any deck, another project deck too, mounts as a
//! plain mount that leaves the session's project as it is; `project: true`
//! makes a project deck the session's project instead (taking the old one
//! off), and unmounting the project leaves it. Changing the project drops
//! the last `workspace.use`, so the result says when the default moves.

use rumu_cards::cards::{
    SELF_MOUNT, default_mount_point, mount_points_overlap, normalize_mount_point,
};
use rumu_cards::deck::{Action, Deck};
use rumu_core::{
    CardMount, CardUnmount, Content, Cx, PathState, RUMU_WORKSPACE, SCRATCH_WORKSPACE, ToolError,
    ToolOutput, ToolResult,
};
use serde_json::json;

use crate::approve::{approve, deck_name};

use super::WorkspacePlugin;

impl WorkspacePlugin {
    /// `deck.unmount`: take `deck` off every point it is mounted at.
    pub(super) fn unmount_deck(&self, state: &PathState, deck: &Deck) -> ToolResult<ToolOutput> {
        let points: Vec<String> = state
            .mounts
            .iter()
            .filter(|m| m.deck == deck.meta.id)
            .map(|m| m.at.clone())
            .collect();
        if points.is_empty() {
            return Err(ToolError::model(format!(
                "`{}` is not mounted",
                deck.meta.name
            )));
        }
        if points.iter().any(|at| at == SELF_MOUNT) {
            return Err(ToolError::model(format!(
                "`{}` is this session's own deck (`{SELF_MOUNT}`) and stays mounted",
                deck.meta.name
            )));
        }
        let effects: Vec<Content> = points
            .iter()
            .map(|at| Content::marker(&CardUnmount { at: at.clone() }))
            .collect();
        let shown: Vec<String> = points.iter().map(|at| format!("`{at}`")).collect();
        let mut text = format!(
            "`{}` is unmounted from {}. Content loaded from it earlier is no longer readable, \
             writable or updated.",
            deck.meta.name,
            shown.join(", ")
        );
        if state.project() == Some(deck.id()) {
            text.push_str(" This session leaves its project.");
            text.push_str(&self.default_moved(state, None));
        }
        Ok(ToolOutput::ok(text).with_effects(effects))
    }

    /// `" The default workspace is now …"` when the branch, moved to
    /// `project`, defaults elsewhere than it does: a project change drops the
    /// last `workspace.use`, so the project's first workspace the branch may
    /// address applies, else `@scratch`.
    fn default_moved(&self, state: &PathState, project: Option<&str>) -> String {
        let after = project
            .into_iter()
            .flat_map(|p| self.workspaces.project(p))
            .find(|n| state.workspaces.iter().any(|w| &w.name == n))
            .unwrap_or_else(|| SCRATCH_WORKSPACE.to_owned());
        if state.default_workspace.as_deref() == Some(after.as_str()) {
            String::new()
        } else {
            format!(" The default workspace is now `{after}`.")
        }
    }

    /// `deck.mount`: mount `deck` at `at` (by default at its name), asking
    /// the user unless a grant covers it. With `project`, a project deck
    /// becomes the session's project: the old one is taken off, and a deck
    /// mounted already is marked where it is.
    pub(super) async fn mount_deck(
        &self,
        state: &PathState,
        deck: &Deck,
        at: Option<&str>,
        project: bool,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if project && !deck.project() {
            return Err(ToolError::model(format!(
                "`{}` is not a project deck: only one can be this session's project",
                deck.meta.name
            )));
        }
        let old = state.project_mount().filter(|_| project);
        if old.is_some_and(|m| m.deck == deck.meta.id) {
            return Err(ToolError::model(format!(
                "`{}` is this session's project already",
                deck.meta.name
            )));
        }
        // The mounts that stay: the old project goes when another comes.
        let staying: Vec<CardMount> = state
            .mounts
            .iter()
            .filter(|m| Some(*m) != old)
            .cloned()
            .collect();
        let at = match staying.iter().find(|m| m.deck == deck.meta.id) {
            Some(same) if project => same.at.clone(),
            Some(same) => {
                return Err(ToolError::model(format!(
                    "`{}` is already mounted at `{}`",
                    deck.meta.name, same.at
                )));
            }
            None => {
                let at = match at {
                    Some(at) => normalize_mount_point(at).map_err(ToolError::model)?,
                    None => default_mount_point(&deck.meta.name, fallback_point(deck), &staying),
                };
                if at == SELF_MOUNT {
                    return Err(ToolError::model(format!(
                        "`{SELF_MOUNT}` is this session's own deck and is always mounted"
                    )));
                }
                if let Some(other) = staying.iter().find(|m| mount_points_overlap(&m.at, &at)) {
                    return Err(ToolError::model(if other.at == at {
                        format!("`{at}` is already mounted")
                    } else {
                        format!("mount point `{at}` overlaps `{}`", other.at)
                    }));
                }
                at
            }
        };
        let request = json!({
            "kind": "deck_mount",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "deck": deck.meta.name,
            "deck_id": deck.meta.id,
            "at": at,
            "project": project,
        });
        if !approve(self.decks(), cx, Action::Mount, deck.id(), request)
            .await
            .map_err(|e| ToolError::model(format!("deck.mount: {e}")))?
        {
            return Err(ToolError::model(format!(
                "the user declined to mount `{}` at `{at}`",
                deck.meta.name
            )));
        }
        let mut out = self.mounted_output(state, deck, &at, project);
        if let Some(old) = old {
            let mut effects = vec![Content::marker(&CardUnmount { at: old.at.clone() })];
            effects.append(&mut out.effects);
            out.effects = effects;
        }
        Ok(out)
    }

    /// The mount result and its markers: the `CardMount` marker, then the
    /// pinned cards it shows.
    fn mounted_output(
        &self,
        state: &PathState,
        deck: &Deck,
        at: &str,
        project: bool,
    ) -> ToolOutput {
        let point = at.trim_start_matches("/cards/");
        let mut text = format!("mounted deck `{}` at `{at}`", deck.meta.name);
        if deck.project() {
            let names = self.workspaces.project(deck.id());
            if names.is_empty() {
                text.push_str(&format!(
                    "\nIt mounts no workspace; add some to `workspaces:` in \
                     `{RUMU_WORKSPACE}:/decks/{}`.",
                    deck.meta.name
                ));
            } else {
                text.push_str(&format!(
                    "\nIts workspaces: {}.",
                    names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if project {
                text.push_str(match state.project_mount() {
                    Some(_) => " It is this session's project now, in place of the old one.",
                    None => " It is this session's project now.",
                });
                text.push_str(&self.default_moved(state, Some(deck.id())));
            } else {
                text.push_str(
                    " This session's project stays as it is (`project: true` would make it \
                     this one).",
                );
            }
        }
        let pinned = deck.pinned().unwrap_or_default();
        if !pinned.is_empty() {
            text.push_str("\nPinned cards:\n");
            for card in &pinned {
                text.push_str(&format!(
                    "## {point}/{}\n{}\n",
                    card.path,
                    card.body.trim_end()
                ));
            }
        }
        // The catalog the catalog note could not carry: it was written before
        // this deck was mounted.
        if let Ok(note) = rumu_cards::catalog::deck_catalog(
            self.decks(),
            deck,
            at,
            &state.assistant,
            rumu_core::now(),
        ) && (!note.entries.is_empty() || !note.folded.is_empty())
        {
            text.push('\n');
            text.push_str(&note.markdown());
        }
        let mut effects = vec![Content::marker(&CardMount::new(
            deck.meta.id.clone(),
            at,
            project,
        ))];
        // The pinned cards shown are uses of their revisions, as the
        // introduction's are: a later change brings the new text.
        effects.extend(pinned.into_iter().map(|card| {
            Content::marker(&rumu_cards::usage::CardImport {
                deck: deck.meta.id.clone(),
                path: card.path,
                rev: card.revision,
            })
        }));
        ToolOutput::ok(text).with_effects(effects)
    }
}

/// The mount point a deck whose name leaves nothing path-safe gets.
pub(crate) fn fallback_point(deck: &Deck) -> &'static str {
    if deck.project() { "project" } else { "deck" }
}
