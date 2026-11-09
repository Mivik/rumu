//! `card_mount`: the branch's card mount table.
//!
//! Mounts are branch state (a note marker on the path), so forks and
//! sub-agents inherit them. Mounting makes a deck readable; writes to it are
//! approved separately (see [`crate::approve`]). The mount result carries the
//! mounted deck's pinned cards and catalog; the unmount result says the deck
//! is no longer readable/writable. Decks are named by id (the listing shows
//! them).
//!
//! `/cards/project` is the session's project: only project decks mount
//! there, mounting one moves the session into it (replacing another), and
//! unmounting it leaves the project. A project deck mounted elsewhere is just
//! a read mount.

use rumu_cards::Cards;
use rumu_cards::cards::{SELF_MOUNT, mount_points_overlap, normalize_mount_point};
use rumu_cards::deck::{Action, Deck, valid_deck_name};
use rumu_core::{
    CardMount, CardUnmount, Content, Cx, PROJECT_MOUNT, PathState, SCRATCH_WORKSPACE, ToolError,
    ToolOutput, ToolResult, WorkspaceUse,
};
use serde_json::{Value, json};

use crate::approve::{allowed, approve, deck_name};
use crate::workspaces::create_project;

use super::WorkspacePlugin;
use super::tools::parse_mount_args;

impl WorkspacePlugin {
    pub(super) async fn tool_card_mount(
        &self,
        state: &PathState,
        args: &Value,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let args = parse_mount_args(args)?;
        let cards = Cards::new(self.decks(), &state.mounts);
        match (
            args.deck.as_deref(),
            args.at.as_deref(),
            args.create.as_deref(),
        ) {
            (None, None, None) => Ok(self.mount_listing(state, &cards, cx).into()),
            (None, None, Some(_)) => Err(ToolError::model(
                "card_mount: `create` needs `at: \"project\"`",
            )),
            (Some(_), _, Some(_)) => Err(ToolError::model(
                "card_mount: `create` cannot be combined with `deck`",
            )),
            (Some(_), None, None) => Err(ToolError::model(
                "card_mount: mounting a deck needs `at` (for example `/cards/user`)",
            )),
            (None, Some(at), None) => self.unmount(state, at),
            (Some(deck), Some(at), None) => self.mount(state, deck, at, cx).await,
            (None, Some(at), Some(name)) => self.create_and_mount(state, name, at, cx).await,
        }
    }

    fn mount_listing(&self, state: &PathState, cards: &Cards, cx: &Cx) -> String {
        let session = cx.session.to_string();
        let mut lines = Vec::new();
        for deck in self.decks().list() {
            let mounted = cards
                .mounts()
                .iter()
                .find(|m| m.deck.id() == deck.id())
                .map(|m| m.mount.at.clone());
            let allowed = |action| allowed(self.decks(), state, &session, action, deck.id());
            let asks: Vec<&str> = [(Action::Mount, "mount"), (Action::Write, "write")]
                .into_iter()
                .filter(|(action, _)| !allowed(*action))
                .map(|(_, name)| name)
                .collect();
            lines.push(format!(
                "{} ({})  {}{}{}{}",
                deck.meta.name,
                deck.meta.id,
                mounted
                    .map(|m| format!("at {m}"))
                    .unwrap_or_else(|| "not mounted".into()),
                if deck.meta.assistant {
                    "  assistant"
                } else if deck.project() {
                    "  project"
                } else {
                    ""
                },
                if asks.is_empty() {
                    String::new()
                } else {
                    format!("  asks the user to {}", asks.join(", "))
                },
                deck.summary()
                    .map(|s| format!("  — {s}"))
                    .unwrap_or_default(),
            ));
        }
        if lines.is_empty() {
            "no decks".into()
        } else {
            lines.join("\n")
        }
    }

    fn unmount(&self, state: &PathState, at: &str) -> ToolResult<ToolOutput> {
        let at = normalize_mount_point(at).map_err(ToolError::model)?;
        if at == SELF_MOUNT {
            return Err(ToolError::model(format!(
                "`{SELF_MOUNT}` is this session's own deck and stays mounted"
            )));
        }
        if !state.mounts.iter().any(|m| m.at == at) {
            return Err(ToolError::model(format!("`{at}` is not mounted")));
        }
        let mut effects = vec![Content::marker(&CardUnmount { at: at.clone() })];
        let text = if at == PROJECT_MOUNT {
            effects.push(Content::marker(&WorkspaceUse {
                name: SCRATCH_WORKSPACE.into(),
            }));
            format!(
                "`{PROJECT_MOUNT}` is unmounted; this session leaves its project and `{SCRATCH_WORKSPACE}` \
                 is the default workspace. Content loaded from it earlier is no longer readable, \
                 writable or updated."
            )
        } else {
            format!(
                "`{at}` is unmounted. Content loaded from it earlier is no longer readable, \
                 writable or updated."
            )
        };
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

    async fn mount(
        &self,
        state: &PathState,
        deck_id: &str,
        at: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if state.asked_by.is_some() {
            return Err(ToolError::model(
                "you are answering a question; this session keeps its mounts",
            ));
        }
        let at = normalize_mount_point(at).map_err(ToolError::model)?;
        let Some(deck) = self.decks().get(deck_id) else {
            return Err(ToolError::model(format!(
                "no deck with id `{deck_id}` (`card_mount` with no arguments lists the decks)"
            )));
        };
        if at == SELF_MOUNT {
            return Err(ToolError::model(format!(
                "`{SELF_MOUNT}` is this session's own deck and is always mounted"
            )));
        }
        self.check_mount_point(state, &deck, &at)?;
        let request = json!({
            "kind": "card_mount",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "deck": deck.meta.name,
            "deck_id": deck.meta.id,
            "at": at,
        });
        if !approve(self.decks(), cx, Action::Mount, deck.id(), request)
            .await
            .map_err(|e| ToolError::model(format!("card_mount: {e}")))?
        {
            return Err(ToolError::model(format!(
                "the user declined to mount `{}` at `{at}`",
                deck.meta.name
            )));
        }
        Ok(self.mounted_output(&deck, &at, &state.assistant))
    }

    /// `card_mount(create: <name>, at: "project")`: create a blank project
    /// deck (with the user's approval) and move into it.
    async fn create_and_mount(
        &self,
        state: &PathState,
        name: &str,
        at: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if state.asked_by.is_some() {
            return Err(ToolError::model(
                "you are answering a question; this session keeps its mounts",
            ));
        }
        let at = normalize_mount_point(at).map_err(ToolError::model)?;
        if at != PROJECT_MOUNT {
            return Err(ToolError::model(
                "card_mount: `create` needs `at: \"project\"`",
            ));
        }
        if !valid_deck_name(name) {
            return Err(ToolError::model(format!(
                "invalid project name `{name}` (rule: `[a-z][a-z0-9-]*`)"
            )));
        }
        let request = json!({
            "kind": "project_create",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "name": name,
        });
        if !approve(self.decks(), cx, Action::Create, "*", request)
            .await
            .map_err(|e| ToolError::model(format!("card_mount: {e}")))?
        {
            return Err(ToolError::model(format!(
                "the user declined to create project `{name}`"
            )));
        }
        let deck = create_project(&self.home, self.decks(), &self.workspaces, name, Vec::new())
            .map_err(|e| ToolError::model(format!("creating project `{name}`: {e}")))?;
        self.check_mount_point(state, &deck, &at)?;
        Ok(self.mounted_output(&deck, &at, &state.assistant))
    }

    /// The mount result and its markers: the `CardMount` marker, then (for a
    /// project) a `WorkspaceUse` marker for its first workspace or `@scratch`.
    fn mounted_output(&self, deck: &Deck, at: &str, assistant: &str) -> ToolOutput {
        let point = at.trim_start_matches("/cards/");
        let mut text = format!("mounted deck `{}` at `{at}`", deck.meta.name);
        if at == PROJECT_MOUNT {
            let names = self.workspaces.project(deck.id());
            if names.is_empty() {
                text.push_str(
                    "\nIt mounts no workspace; `@scratch` is the default. Add some through \
                     `@rumu:/project/workspaces`.",
                );
            } else {
                text.push_str(&format!(
                    "\nIts workspaces (default first): {}",
                    names
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        let pinned = deck.pinned().unwrap_or_default();
        if !pinned.is_empty() {
            text.push_str("\nPinned cards:\n");
            for card in pinned {
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
