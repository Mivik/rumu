//! The `@rumu:/cards` part of the `@rumu` workspace: read/write/edit on the
//! cards of the branch's mounted decks, with read-before-write (CAS) backed
//! by git history.

use rumu_cards::deck::Action;
use rumu_cards::usage::CardOp;
use rumu_cards::{Cards, card, usage};
use rumu_core::{Cx, Effect, ToolError, ToolOutput, ToolResult};

use super::WorkspacePlugin;
use super::patch;
use super::time::iso_local;
use super::tools::{card_load_err, invalid_card_ref, no_card_err, slice_lines};
use crate::approve::{approve, deck_name};

/// The git author of a tool write: the assistant deck, session, and the
/// assistant node that asked for the call.
pub(super) fn author_of(cx: &Cx, decks: &rumu_cards::Decks) -> rumu_cards::deck::Author {
    let deck_id = cx
        .state()
        .map(|s| s.assistant.clone())
        .unwrap_or_else(|_| rumu_cards::deck::USER.to_owned());
    let name = decks
        .get(&deck_id)
        .map(|d| d.meta.name)
        .unwrap_or_else(|| deck_id.clone());
    let session = Some(cx.session.0.clone());
    let node = cx.call().and_then(|call| {
        cx.path().ok()?.iter().find_map(|n| match &n.body {
            rumu_core::NodeBody::Assistant { tool_calls, .. }
                if tool_calls.iter().any(|c| c.call_id == call.id) =>
            {
                Some(n.id.0.clone())
            }
            _ => None,
        })
    });
    rumu_cards::deck::Author {
        id: deck_id,
        name,
        session,
        node,
    }
}

impl WorkspacePlugin {
    /// The mounted card space of the branch.
    pub(super) fn cards(&self, cx: &Cx) -> ToolResult<Cards> {
        let state = cx.state()?;
        Ok(Cards::new(self.decks(), &state.mounts))
    }

    pub(super) async fn card_read(
        &self,
        public: &str,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let cards = self.cards(cx)?;
        if let Some(err) = invalid_card_ref(&cards, public) {
            return Err(err);
        }
        let existing = match cards.load(public) {
            Ok(Some(c)) => c,
            Ok(None) => {
                let known: Vec<String> = cards.walk().unwrap_or_default();
                return Err(no_card_err(public, &known));
            }
            Err(e) => return Err(card_load_err(public, e)),
        };
        if existing.dangling.is_some() {
            return Err(ToolError::model(format!(
                "card `{}` links to a card that no longer exists",
                rumu_cards::card_uri(public)
            )));
        }
        let view = card::view_of(&existing.card);
        let view_rev = card::view_rev_of(&view);
        // The CAS base is the commit of the deck that holds the content (the
        // target's deck for a link).
        let commit = self
            .decks()
            .get(&existing.deck)
            .and_then(|deck| deck.head().ok().flatten());
        let effect = record_card_use(
            cx,
            &existing.deck,
            &existing.content_path,
            &existing.card.revision,
            &view_rev,
            commit.as_deref(),
            CardOp::Read,
        );
        let header = card::CardReadHeader {
            deck: existing.deck.clone(),
            path: existing.content_path.clone(),
        };
        let link_header = existing.link.is_some().then(|| {
            format!(
                "(`{}` links to `{}`; write the target to change it)\n",
                rumu_cards::card_uri(public),
                rumu_cards::card_uri(&existing.card.path)
            )
        });
        match slice_lines(view, offset, limit) {
            Ok(text) => {
                // A successful model read is use; a read through a link counts
                // for the target (`existing` already holds its deck and path).
                if let Some(deck) = self.decks().get(&existing.deck)
                    && let Some(id) = existing.card.id()
                {
                    rumu_cards::usage_index::bump(
                        &deck.dir,
                        id,
                        rumu_cards::usage_index::READ_WEIGHT,
                        rumu_core::now(),
                    );
                }
                let text = match link_header {
                    Some(h) => format!("{h}{text}"),
                    None => text,
                };
                Ok(ToolOutput::ok(text).with_data(&header).with_effect(effect))
            }
            Err(e) => Err(ToolError::model(e)),
        }
    }

    /// The `list @rumu:/cards[/<prefix>]` output. At the root it names the
    /// mount points; below one it lists its cards.
    pub(super) async fn card_list(&self, prefix: &str, cx: &Cx) -> ToolResult<ToolOutput> {
        let cards = self.cards(cx)?;
        if cards.is_empty() {
            return Ok("no decks mounted; use card_mount to mount one".into());
        }
        rumu_cards::list_text(&cards, prefix, super::tools::LIST_LIMIT)
    }

    /// Write (or move, with `from`) a card's editable view. Overwriting an
    /// existing card requires the latest `card_use` on this branch to carry
    /// the card's current `view_rev` — read it first.
    pub(super) async fn card_write(
        &self,
        public: &str,
        view_text: &str,
        op: CardOp,
        from: Option<&str>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let from = from.map(|f| {
            f.strip_prefix(rumu_cards::cards::LINK_PREFIX)
                .or_else(|| f.strip_prefix("/cards/"))
                .unwrap_or(f)
        });
        let cards = self.cards(cx)?;
        let state = cx.state()?;
        if let Some(err) = invalid_card_ref(&cards, public) {
            return Err(err);
        }
        let existing = match cards.raw(public) {
            Ok(o) => o,
            Err(e) => return Err(card_load_err(public, e)),
        };
        if let Some(old) = &existing
            && old.link().is_some()
        {
            return Err(ToolError::model(format!(
                "card `{}` is a link; write its target instead",
                rumu_cards::card_uri(public)
            )));
        }
        let source = match from.filter(|src| *src != public) {
            Some(src) => {
                if let Some(err) = invalid_card_ref(&cards, src) {
                    return Err(err);
                }
                let Some(source) = cards.raw(src).map_err(|e| card_load_err(src, e))? else {
                    return Err(no_card_err(src, &[]));
                };
                if source.link().is_some() {
                    return Err(ToolError::model(format!(
                        "cannot move `{}`: it is a link",
                        rumu_cards::card_uri(src)
                    )));
                }
                if let Some(err) = self.ensure_fresh_read(&cards, src, &source, cx) {
                    return Err(err);
                }
                if existing.is_some() {
                    return Err(ToolError::model(format!(
                        "move destination `{}` already exists",
                        rumu_cards::card_uri(public)
                    )));
                }
                Some(source)
            }
            None => None,
        };
        // Post-validator: nothing touches disk until the view parses clean.
        let new_card = match card::card_from_view(public, view_text) {
            Ok(c) => c,
            Err(e) => return Err(ToolError::model(e)),
        };
        if let Some(old) = &existing
            && let Some(err) = self.ensure_fresh_read(&cards, public, old, cx)
        {
            return Err(err);
        }
        // Linking another deck's card shares it with every session mounting
        // this deck; that needs the user's approval unless it is the
        // assistant's own card.
        if let Some(target) = new_card.link() {
            let (target_deck, target_path) = cards
                .resolve_link(target)
                .map_err(|e| ToolError::model(format!("link: {e}")))?;
            let dest = cards
                .split(public)
                .map(|(m, p)| (m.deck.id().to_owned(), p.to_owned()));
            if dest.as_ref() == Some(&(target_deck.clone(), target_path.clone())) {
                return Err(ToolError::model("a card cannot link to itself"));
            }
            let link_deck = cards.deck_for(public).map(|d| d.meta.id);
            if let Some(from_deck) = &link_deck
                && !self.decks().link_allowed(from_deck, &target_deck)
            {
                return Err(ToolError::model(
                    "a card in a private deck can only be linked from its own deck",
                ));
            }
            // Links are one level deep, even when the target deck is not
            // mounted right now.
            if let Some(deck) = self.decks().get(&target_deck)
                && let Ok(Some(text)) = deck.read(&target_path)
                && card::parse_card(&target_path, &text).link().is_some()
            {
                return Err(ToolError::model(
                    "the target is itself a link; links are one level deep",
                ));
            }
            let stored = format!("{target_deck}:{target_path}");
            let changed = existing.as_ref().and_then(|c| c.link()) != Some(stored.as_str());
            // Only a link into another deck shares anything new.
            if changed && Some(&target_deck) != link_deck.as_ref() {
                let request = serde_json::json!({
                    "kind": "card_link",
                    "from": deck_name(self.decks(), &state.assistant),
                    "from_id": state.assistant,
                    "to": deck_name(self.decks(), &target_deck),
                    "to_id": target_deck,
                    "target": target,
                    "path": public,
                });
                if !approve(self.decks(), cx, Action::Link, &target_deck, request)
                    .await
                    .map_err(|e| ToolError::model(format!("link: {e}")))?
                {
                    return Err(ToolError::model(format!(
                        "the user declined to link `{target}` into this card"
                    )));
                }
            }
        }
        // Writing to (or moving out of) another assistant's or a shared deck
        // needs the user's approval, deck by deck.
        let source_deck = from
            .filter(|src| *src != public)
            .and_then(|src| cards.deck_for(src))
            .map(|d| d.meta.id);
        let target_deck = cards.deck_for(public).map(|d| d.meta.id);
        let mut touched: Vec<String> = target_deck.into_iter().chain(source_deck).collect();
        touched.dedup();
        for deck in touched {
            let request = serde_json::json!({
                "kind": "card_write",
                "from": deck_name(self.decks(), &state.assistant),
                "from_id": state.assistant,
                "deck": deck_name(self.decks(), &deck),
                "deck_id": deck,
                "path": public,
                "move_from": from.filter(|src| *src != public),
                "content": view_text,
            });
            if !approve(self.decks(), cx, Action::Write, &deck, request)
                .await
                .map_err(|e| ToolError::model(format!("write: {e}")))?
            {
                return Err(ToolError::model(format!(
                    "the user declined this write to deck `{}`",
                    deck_name(self.decks(), &deck)
                )));
            }
        }
        let author = author_of(cx, self.decks());
        // The view this write was checked against (a move's source, else the
        // destination); the store re-checks it under its write lock, after
        // any wait for the user's approval.
        let base = source
            .as_ref()
            .or(existing.as_ref())
            .map(|c| card::view_rev_of(&card::view_of(c)))
            .unwrap_or_default();
        if let Err(e) = cards.write(public, view_text, from, Some(&base), &author) {
            return Err(ToolError::model(format!("write failed: {e}")));
        }
        let current = cards.raw(public).ok().flatten();
        let view = current.as_ref().map(card::view_of).unwrap_or_default();
        let view_rev = card::view_rev_of(&view);
        let rev = current
            .as_ref()
            .map(|c| c.revision.clone())
            .unwrap_or_default();
        let commit = cards.head(public).unwrap_or_default();
        let (deck_id, content_path) = cards
            .split(public)
            .map(|(m, p)| (m.deck.id().to_owned(), p.to_owned()))
            .unwrap_or_default();
        let effect = record_card_use(
            cx,
            &deck_id,
            &content_path,
            &rev,
            &view_rev,
            commit.as_deref(),
            op,
        );
        // The UI diff is view → view: what the model read against what it wrote.
        let patch = match &existing {
            Some(cur) => patch::update(card::view_of(cur), view).await,
            None => patch::create(),
        };
        let mut text = format!("wrote cards/{public}.md (rev {rev}, view rev {view_rev})");
        if let Some((mounted, _)) = cards.split(public).filter(|_| new_card.link().is_none()) {
            text.push_str(&link_warnings(&mounted.deck, &content_path, &new_card.body));
        }
        Ok(ToolOutput::ok(text).with_data(&patch).with_effect(effect))
    }

    /// Read-before-write concurrency: the latest `card_use` for the card's
    /// `(deck, path)` on the branch path must carry the card's current
    /// on-disk `view_rev`. A refusal lists the git commits since the read that
    /// touch the path, and the diff from the read version to the current one.
    pub(super) fn ensure_fresh_read(
        &self,
        cards: &Cards,
        public: &str,
        current: &rumu_cards::Card,
        cx: &Cx,
    ) -> Option<ToolError> {
        let shown = rumu_cards::card_uri(public);
        let on_disk = card::view_rev_of(&card::view_of(current));
        let (deck_id, path) = cards.split(public)?;
        let seen = cx
            .path()
            .ok()
            .and_then(|nodes| usage::latest_tool_use(&nodes, deck_id.deck.id(), path));
        let message = match seen {
            None => format!("card `{shown}` exists; read it first (`read {shown}`) before writing"),
            Some(use_) if use_.view_rev.as_deref() != Some(on_disk.as_str()) => {
                let mut message = format!(
                    "card `{shown}` changed since you last read it (you saw view rev {}, on disk \
                     {on_disk}).",
                    use_.view_rev.as_deref().unwrap_or("?")
                );
                let mut listed = false;
                if let Some(deck) = cards.deck_for(public)
                    && let Some(commit) = use_.commit.as_deref()
                    && let Ok(changes) = deck.touching(path, Some(commit))
                {
                    if !changes.is_empty() {
                        message.push_str("\nChanges since then:");
                        listed = true;
                        for c in &changes {
                            let (op, path) = c.op_path();
                            let who = match &c.session {
                                Some(session) => {
                                    format!("{} (session {session})", c.author)
                                }
                                None => c.author.clone(),
                            };
                            message.push_str(&format!(
                                "\n- {op} {path} by {who} at {}",
                                iso_local(c.at * 1000)
                            ));
                        }
                    }
                    if let Ok(Some(old_text)) = deck.text_at(commit, &format!("{path}.md"))
                        && let Ok(Some(now_text)) = cards.current_text(public)
                    {
                        let old_view = card::view_of(&card::parse_card(path, &old_text));
                        let now_view = card::parse_card(path, &now_text);
                        let diff = rumu_cards::diff::unified(&old_view, &card::view_of(&now_view));
                        if !diff.trim().is_empty() {
                            message.push_str("\nDiff (seen → current):\n");
                            message.push_str(&diff);
                        }
                    }
                }
                let _ = listed;
                message.push_str(
                    "\nre-read the card and merge your edit; there is no automatic merge.",
                );
                message
            }
            _ => return None,
        };
        Some(ToolError::model(message))
    }
}

/// The `card_use` marker recording a tool read/write (the result itself is
/// the snapshot). Keyed by the card's deck and path; the commit is the CAS
/// base.
fn record_card_use(
    cx: &Cx,
    deck: &str,
    path: &str,
    rev: &str,
    view_rev: &str,
    commit: Option<&str>,
    op: CardOp,
) -> Effect {
    usage::card_use(
        deck,
        path,
        rev,
        view_rev,
        commit,
        op,
        cx.call().map(|c| c.id.as_str()),
    )
}

/// Warnings for the body links of a card just written at deck-local `path`:
/// relative links that name no card in its deck, and mount paths, which break
/// wherever the deck is mounted elsewhere. Empty when every link is fine.
fn link_warnings(deck: &rumu_cards::Deck, path: &str, body: &str) -> String {
    let mut broken = Vec::new();
    let mut mounted = Vec::new();
    for (href, link) in card::body_links(path, body) {
        match link {
            Ok(target) => {
                let exists = rumu_cards::valid_card_path(&target)
                    && matches!(deck.read(&target), Ok(Some(_)));
                if !exists && !broken.contains(&href) {
                    broken.push(href);
                }
            }
            Err(()) if !mounted.contains(&href) => mounted.push(href),
            Err(()) => {}
        }
    }
    let list = |hrefs: &[&str]| {
        hrefs
            .iter()
            .map(|h| format!("`{h}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut out = String::new();
    if !broken.is_empty() {
        out.push_str(&format!(
            "\nThese links name no card in this deck: {}.",
            list(&broken)
        ));
    }
    if !mounted.is_empty() {
        out.push_str(&format!(
            "\nThese links use a mount path, which differs wherever the deck is mounted: {}. \
             Link to a card in the same deck by its path relative to this card (`./x`, `../d/y`); \
             for a card in another deck, link to a link card in this deck.",
            list(&mounted)
        ));
    }
    out
}
