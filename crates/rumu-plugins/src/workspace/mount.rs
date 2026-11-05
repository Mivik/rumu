//! `card_mount`: the branch's card mount table.
//!
//! Mounts are branch state (a note marker on the path), so forks and
//! sub-agents inherit them. Mounting makes a deck readable; writes to it are
//! approved separately (see [`crate::approve`]). The mount result carries the
//! mounted deck's pinned cards and catalog; the unmount result says the deck
//! is no longer readable/writable. Decks are named by id (the listing shows
//! them).

use rumu_cards::Cards;
use rumu_cards::cards::{SELF_MOUNT, mount_points_overlap, normalize_mount_point};
use rumu_cards::deck::Action;
use rumu_core::{
    CardMount, CardUnmount, Content, Cx, PathState, ToolError, ToolOutput, ToolResult,
};
use serde_json::{Value, json};

use crate::approve::{approve, deck_name};

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
        match (args.deck.as_deref(), args.at.as_deref()) {
            (None, None) => Ok(self.mount_listing(state, &cards, cx).into()),
            (None, Some(at)) => self.unmount(state, at),
            (Some(deck), Some(at)) => self.mount(state, deck, at, cx).await,
            (Some(_), None) => Err(ToolError::model(
                "card_mount: mounting a deck needs `at` (for example `/cards/user`)",
            )),
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
            let allowed = |action| {
                self.decks()
                    .allowed(action, &session, &state.assistant, deck.id())
            };
            let asks: Vec<&str> = [(Action::Mount, "mount"), (Action::Write, "write")]
                .into_iter()
                .filter(|(action, _)| !allowed(*action))
                .map(|(_, name)| name)
                .collect();
            lines.push(format!(
                "{} ({})  {}{}{}",
                deck.meta.name,
                deck.meta.id,
                mounted
                    .map(|m| format!("at {m}"))
                    .unwrap_or_else(|| "not mounted".into()),
                if deck.meta.assistant {
                    "  assistant"
                } else {
                    ""
                },
                if asks.is_empty() {
                    String::new()
                } else {
                    format!("  asks the user to {}", asks.join(", "))
                }
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
        Ok(ToolOutput::ok(format!(
            "`{at}` is unmounted. Content loaded from it earlier is no longer readable, writable \
             or updated."
        ))
        .with_effect(Content::marker(&CardUnmount { at })))
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
        if let Some(other) = state
            .mounts
            .iter()
            .find(|m| mount_points_overlap(&m.at, &at))
        {
            return Err(ToolError::model(if other.at == at {
                format!("`{at}` is already mounted")
            } else {
                format!("mount point `{at}` overlaps `{}`", other.at)
            }));
        }
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
        let mounted = CardMount {
            deck: deck.meta.id.clone(),
            at: at.clone(),
        };
        let point = at.trim_start_matches("/cards/");
        let mut text = format!("mounted deck `{}` at `{at}`", deck.meta.name);
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
        if let Ok(note) = rumu_cards::catalog::deck_catalog(
            self.decks(),
            &deck,
            &at,
            &state.assistant,
            rumu_core::now(),
        ) && (!note.entries.is_empty() || !note.folded.is_empty())
        {
            text.push('\n');
            text.push_str(&note.markdown());
        }
        Ok(ToolOutput::ok(text).with_effect(Content::marker(&mounted)))
    }
}
