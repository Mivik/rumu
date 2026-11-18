//! The branch's card mount table, changed by the `deck.mount` and
//! `deck.unmount` actions of `@rumu:/decks/<name>`.
//!
//! Mounts are branch state (a note marker on the path), so forks and
//! sub-agents inherit them. Mounting makes a deck readable; writes to it are
//! approved separately (see [`crate::approve`]). The mount result carries the
//! mounted deck's pinned cards and catalog; the unmount result says the deck
//! is no longer readable/writable.
//!
//! `/cards/project` is the session's project: only project decks mount
//! there, mounting one moves the session into it (replacing another), and
//! unmounting it leaves the project. A project deck mounted elsewhere is just
//! a read mount.

use rumu_cards::cards::{SELF_MOUNT, mount_points_overlap, normalize_mount_point};
use rumu_cards::deck::{Action, Deck};
use rumu_core::{
    CardMount, CardUnmount, Content, Cx, PROJECT_MOUNT, PathState, RUMU_WORKSPACE,
    SCRATCH_WORKSPACE, ToolError, ToolOutput, ToolResult, WorkspaceUse,
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
        let mut effects: Vec<Content> = points
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
        if points.iter().any(|at| at == PROJECT_MOUNT) {
            effects.push(Content::marker(&WorkspaceUse {
                name: SCRATCH_WORKSPACE.into(),
            }));
            text.push_str(&format!(
                " This session leaves its project; `{SCRATCH_WORKSPACE}` is the default workspace."
            ));
        }
        Ok(ToolOutput::ok(text).with_effects(effects))
    }

    /// Refuse a mount point another deck occupies (unless it is the project,
    /// which a later project mount replaces), overlaps, or that is invalid.
    fn check_mount_point(&self, state: &PathState, deck: &Deck, at: &str) -> ToolResult<()> {
        if at == PROJECT_MOUNT && !deck.project() {
            return Err(ToolError::model(format!(
                "`{PROJECT_MOUNT}` holds only project decks; `{}` is not one",
                deck.meta.name
            )));
        }
        if let Some(other) = state
            .mounts
            .iter()
            .find(|m| mount_points_overlap(&m.at, at))
        {
            if at == PROJECT_MOUNT && other.at == PROJECT_MOUNT {
                return Ok(()); // replaced by the new project mount
            }
            return Err(ToolError::model(if other.at == at {
                format!("`{at}` is already mounted")
            } else {
                format!("mount point `{at}` overlaps `{}`", other.at)
            }));
        }
        Ok(())
    }

    /// `deck.mount`: mount `deck` at `at` (`project` moves the session into
    /// it), asking the user unless a grant covers it.
    pub(super) async fn mount_deck(
        &self,
        state: &PathState,
        deck: &Deck,
        at: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let at = normalize_mount_point(at).map_err(ToolError::model)?;
        if at == SELF_MOUNT {
            return Err(ToolError::model(format!(
                "`{SELF_MOUNT}` is this session's own deck and is always mounted"
            )));
        }
        self.check_mount_point(state, deck, &at)?;
        let request = json!({
            "kind": "deck_mount",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "deck": deck.meta.name,
            "deck_id": deck.meta.id,
            "at": at,
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
        Ok(self.mounted_output(deck, &at, &state.assistant))
    }

    /// The mount result and its markers: the `CardMount` marker, then (for a
    /// project) a `WorkspaceUse` marker for its first workspace or `@scratch`.
    fn mounted_output(&self, deck: &Deck, at: &str, assistant: &str) -> ToolOutput {
        let point = at.trim_start_matches("/cards/");
        let mut text = format!("mounted deck `{}` at `{at}`", deck.meta.name);
        if at == PROJECT_MOUNT {
            let names = self.workspaces.project(deck.id());
            if names.is_empty() {
                text.push_str(&format!(
                    "\nIt mounts no workspace; `@scratch` is now the default workspace. Add some \
                     to `workspaces:` in `{RUMU_WORKSPACE}:/decks/{}`.",
                    deck.meta.name
                ));
            } else {
                text.push_str(&format!(
                    "\nIts workspaces: {}; `{}` is now the default workspace.",
                    names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    names[0]
                ));
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
        if let Ok(note) =
            rumu_cards::catalog::deck_catalog(self.decks(), deck, at, assistant, rumu_core::now())
            && (!note.entries.is_empty() || !note.folded.is_empty())
        {
            text.push('\n');
            text.push_str(&note.markdown());
        }
        let mut effects = vec![Content::marker(&CardMount {
            deck: deck.meta.id.clone(),
            at: at.to_owned(),
        })];
        // The pinned cards shown are uses of their revisions, as the
        // introduction's are: a later change brings the new text.
        effects.extend(pinned.into_iter().map(|card| {
            Content::marker(&rumu_cards::usage::CardImport {
                deck: deck.meta.id.clone(),
                path: card.path,
                rev: card.revision,
            })
        }));
        if at == PROJECT_MOUNT {
            let first = self
                .workspaces
                .project(deck.id())
                .into_iter()
                .next()
                .unwrap_or_else(|| SCRATCH_WORKSPACE.to_owned());
            effects.push(Content::marker(&WorkspaceUse { name: first }));
        }
        ToolOutput::ok(text).with_effects(effects)
    }
}
