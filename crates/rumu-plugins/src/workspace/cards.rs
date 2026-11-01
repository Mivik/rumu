//! The `@rumu:/cards/<id>` part of the `@rumu` workspace: read/write/edit on
//! cards' editable views over the home library (`RUMU_HOME/cards`,
//! [`FsCardStore`]).

use rumu_cards::store::FsCardStore;
use rumu_cards::usage::CardOp;
use rumu_cards::{card, library, usage};
use rumu_core::{Cx, Effect, ToolError, ToolOutput, ToolResult};

use super::WorkspacePlugin;
use super::patch;
use super::tools::{card_load_err, invalid_card_id, no_card_err, slice_lines};

impl WorkspacePlugin {
    /// The home card library behind `@rumu:/cards`.
    pub(super) fn card_store(&self) -> FsCardStore {
        FsCardStore::home(&self.home)
    }

    pub(super) async fn card_read(
        &self,
        id: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if let Some(err) = invalid_card_id(id) {
            return Err(err);
        }
        let store = self.card_store();
        let existing = match library::load_card(&store, id) {
            Ok(Some(c)) => c,
            Ok(None) => {
                let known = store.walk().unwrap_or_default();
                return Err(no_card_err(id, &known));
            }
            Err(e) => return Err(card_load_err(id, e)),
        };
        let view = card::view_of(&existing);
        let view_rev = card::view_rev_of(&view);
        let effect = record_card_use(cx, id, &existing.revision, &view_rev, CardOp::Read);
        match slice_lines(view, offset, limit) {
            Ok(text) => Ok(ToolOutput::ok(text).with_effect(effect)),
            Err(e) => Err(ToolError::model(e).with_effect(effect)),
        }
    }

    /// The `list @rumu:/cards[/<prefix>]` output: one
    /// `@rumu:/cards/<id> — title — summary` line per card.
    pub(super) async fn card_list(&self, prefix: &str) -> ToolResult<ToolOutput> {
        rumu_cards::list_text(&self.card_store(), prefix, super::tools::LIST_LIMIT)
    }

    /// Write (or apply an already-replaced edit to) a card's editable view.
    /// `op` is recorded on the use marker. Overwriting an existing card
    /// requires the latest `card_use` on this branch to carry the card's
    /// current `view_rev` — read it first.
    pub(super) async fn card_write(
        &self,
        id: &str,
        view_text: &str,
        op: CardOp,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if let Some(err) = invalid_card_id(id) {
            return Err(err);
        }
        let store = self.card_store();
        let existing = match library::load_card(&store, id) {
            Ok(o) => o,
            Err(e) => return Err(card_load_err(id, e)),
        };
        if let Some(cur) = &existing
            && let Some(err) = self.ensure_fresh_read(id, cur, cx)
        {
            return Err(err);
        }
        // Post-validator: nothing touches disk until the view parses clean.
        let new_card = match card::card_from_view(id, view_text) {
            Ok(c) => c,
            Err(e) => return Err(ToolError::model(e)),
        };
        if let Err(e) = library::write_card(&store, &new_card) {
            return Err(ToolError::model(format!("write failed: {e}")));
        }
        let view = card::view_of(&new_card);
        let view_rev = card::view_rev_of(&view);
        let effect = record_card_use(cx, id, &new_card.revision, &view_rev, op);
        // The UI diff is view → view: what the model read against what it wrote.
        let patch = match &existing {
            Some(cur) => patch::update(card::view_of(cur), view).await,
            None => patch::create(),
        };
        Ok(ToolOutput::ok(format!(
            "wrote cards/{id}.md (rev {}, view rev {view_rev})",
            new_card.revision
        ))
        .with_data(&patch)
        .with_effect(effect))
    }

    /// Read-before-write concurrency: the latest `card_use` for the card on
    /// the branch path must carry the card's current on-disk `view_rev`.
    /// Returns the refusal when the write must be rejected.
    pub(super) fn ensure_fresh_read(
        &self,
        id: &str,
        current: &rumu_cards::Card,
        cx: &Cx,
    ) -> Option<ToolError> {
        let shown = rumu_cards::card_uri(id);
        let on_disk = card::view_rev_of(&card::view_of(current));
        let seen = cx
            .path()
            .ok()
            .and_then(|nodes| usage::latest_tool_use(&nodes, id))
            .and_then(|u| u.view_rev);
        let message = match seen {
            None => format!(
                "card `{shown}` exists; read it first \
                 (`read {shown}`) before writing"
            ),
            Some(v) if v != on_disk => format!(
                "card `{shown}` changed since you last read it \
                 (you saw view rev {v}, on disk {on_disk}); re-read it and merge \
                 your edit"
            ),
            _ => return None,
        };
        Some(ToolError::model(message))
    }
}

/// The `card_use` marker recording a tool read/write (the result itself is
/// the snapshot).
fn record_card_use(cx: &Cx, id: &str, rev: &str, view_rev: &str, op: CardOp) -> Effect {
    usage::card_use(id, rev, view_rev, op, cx.call().map(|c| c.id.as_str()))
}
