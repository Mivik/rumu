//! `@rumu:/history`: the git history of the mounted decks, addressed by path.
//!
//! - `list /history` — newest first, one line per commit over every mounted
//!   deck: short commit, path, op, author, time;
//! - `list /history/cards/<path>` — the versions at that path (moves are not
//!   followed: a moved card is a new card);
//! - `read /history/cards/<path>@<commit>` — one version's header and its diff
//!   from the previous version.
//!
//! A deleted card keeps its old path here: `<path>` is looked up in the
//! history of the deck mounted there. Only mounted decks are shown.

use rumu_cards::git::Commit;
use rumu_cards::{Cards, card};
use rumu_core::{Cx, ToolError, ToolOutput, ToolResult, Utf8UnixPath};

use super::WorkspacePlugin;
use super::time::iso_local;
use super::tools::slice_lines;

const TIMELINE_LIMIT: usize = 100;

impl WorkspacePlugin {
    pub(super) async fn history_list(&self, abs: &Utf8UnixPath, cx: &Cx) -> ToolResult<ToolOutput> {
        let cards = self.cards(cx)?;
        let rest = history_rest(abs);
        if rest.is_empty() {
            return self.history_timeline(&cards).map(Into::into);
        }
        if let Some(path) = rest.strip_prefix("cards/") {
            let (deck, path) = card_at(&cards, path)?;
            return versions(&cards, &deck, &path).map(Into::into);
        }
        Err(ToolError::model(
            "history: use `@rumu:/history` or `@rumu:/history/cards/<path>`",
        ))
    }

    pub(super) async fn history_read(
        &self,
        abs: &Utf8UnixPath,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let cards = self.cards(cx)?;
        let rest = history_rest(abs);
        let Some(rest) = rest.strip_prefix("cards/") else {
            return Err(ToolError::model(
                "history read: use `@rumu:/history/cards/<path>@<commit>`",
            ));
        };
        let Some((path, rev)) = rest.rsplit_once('@') else {
            return Err(ToolError::model(
                "history read: use `@rumu:/history/cards/<path>@<commit>`",
            ));
        };
        let (deck, path) = card_at(&cards, path)?;
        let Some(commit) = deck
            .touching(&path, None)
            .map_err(deck_err)?
            .into_iter()
            .find(|c| c.hash == rev || c.short == rev)
        else {
            return Err(ToolError::model(format!(
                "no version `{rev}` of `{path}` (`list @rumu:/history/cards/{path}` lists them)"
            )));
        };
        let text = render_version(&cards, &deck, &path, &commit)?;
        match slice_lines(text, offset, limit) {
            Ok(text) => Ok(ToolOutput::ok(text)),
            Err(e) => Err(ToolError::model(e)),
        }
    }

    /// `list /history`: the timeline of the mounted decks, newest first.
    fn history_timeline(&self, cards: &Cards) -> ToolResult<String> {
        let mut rows: Vec<(i64, String)> = Vec::new();
        for mounted in cards.mounts() {
            for commit in mounted.deck.commits(TIMELINE_LIMIT).map_err(deck_err)? {
                if !commit.touches_cards() {
                    continue;
                }
                rows.push((commit.at, timeline_line(cards, &mounted.deck, &commit)));
            }
        }
        rows.sort_by_key(|r| std::cmp::Reverse(r.0));
        rows.truncate(TIMELINE_LIMIT);
        if rows.is_empty() {
            return Ok("no card history in the mounted decks".into());
        }
        Ok(rows
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// One path's versions in one deck, newest first.
fn versions(cards: &Cards, deck: &rumu_cards::Deck, path: &str) -> ToolResult<String> {
    let mut rows: Vec<(i64, String)> = Vec::new();
    for commit in deck.touching(path, None).map_err(deck_err)? {
        rows.push((commit.at, version_line(cards, deck, &commit, path)));
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    if rows.is_empty() {
        return Ok("no versions in the mounted decks".into());
    }
    Ok(rows
        .into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n"))
}

fn history_rest(abs: &Utf8UnixPath) -> &str {
    abs.strip_prefix("/history")
        .map(Utf8UnixPath::as_str)
        .unwrap_or("")
        .trim_matches('/')
}

/// The deck and deck-local path behind a public card path. The path must have
/// history in that deck (the card may have been deleted or moved away).
fn card_at(cards: &Cards, path: &str) -> ToolResult<(rumu_cards::Deck, String)> {
    let Some((mounted, rel)) = cards.split(path) else {
        return Err(ToolError::model(format!(
            "`{path}` is not under a mounted deck"
        )));
    };
    let deck = mounted.deck.clone();
    if deck.repo().log_path(rel).map_err(deck_err)?.is_some() {
        return Ok((deck, rel.to_owned()));
    }
    Err(ToolError::model(format!("no card history at `{path}`")))
}

/// A card path at a commit, as the model sees it: under its mount point.
fn shown(cards: &Cards, deck: &rumu_cards::Deck, path: &str) -> String {
    cards
        .public_path(deck.id(), path)
        .unwrap_or_else(|| format!("{}:{path}", deck.meta.name))
}

/// One version's header and diff (or full text) for `read`.
fn render_version(
    cards: &Cards,
    deck: &rumu_cards::Deck,
    path: &str,
    commit: &Commit,
) -> ToolResult<String> {
    let (op, _) = commit.op_path();
    let file = format!("{path}.md");
    let content = if op == "delete" {
        String::new()
    } else {
        deck.text_at(&commit.hash, &file)
            .map_err(deck_err)?
            .unwrap_or_default()
    };
    let display = shown(cards, deck, path);
    let mut text = format!(
        "card {display} · {op} by {} at {}",
        commit.author,
        iso_local(commit.at * 1000)
    );
    // The previous version at the same path, oldest-to-newest.
    let mut history = deck.touching(path, None).map_err(deck_err)?;
    history.reverse();
    let index = history.iter().position(|c| c.hash == commit.hash);
    let previous = index
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| history.get(i));
    match previous {
        Some(previous) => {
            let old = deck
                .text_at(&previous.hash, &file)
                .map_err(deck_err)?
                .unwrap_or_default();
            let diff = rumu_cards::diff::unified(
                &view_text(cards, path, &old),
                &view_text(cards, path, &content),
            );
            text.push_str("\n\n");
            text.push_str(&diff);
        }
        None => {
            text.push_str("\n\n");
            text.push_str(&view_text(cards, path, &content));
        }
    }
    Ok(text)
}

/// The editable view of a version. A link shows its target's path, never the
/// stored deck id and path.
fn view_text(cards: &Cards, path: &str, text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut card = card::parse_card(path, text);
    if let Some((deck, target_path)) = card
        .link_target()
        .map(|(d, p)| (d.to_owned(), p.to_owned()))
    {
        let shown = if cards.public_path(&deck, &target_path).is_some() {
            format!(
                "{}{}",
                rumu_cards::cards::LINK_PREFIX,
                cards.display_path(&deck, &target_path)
            )
        } else if cards.deck_exists(&deck) {
            // Not mounted: `<deck name>:<path>` is still informative.
            cards.display_path(&deck, &target_path)
        } else {
            "(a card that no longer exists)".into()
        };
        card.meta
            .insert("link".into(), serde_json::Value::String(shown));
    }
    card::view_of(&card)
}

/// The path a commit is about: for a move its destination, else its first
/// changed card path, else the subject's.
fn commit_path(commit: &Commit) -> String {
    let (op, rest) = commit.op_path();
    if op == "move"
        && let Some((_, to)) = rest.split_once(" -> ")
    {
        return to.to_owned();
    }
    commit.changed.first().cloned().unwrap_or(rest)
}

fn timeline_line(cards: &Cards, deck: &rumu_cards::Deck, commit: &Commit) -> String {
    let (op, _) = commit.op_path();
    format!(
        "{}  {}  {op}  {}  {}",
        commit.short,
        shown(cards, deck, &commit_path(commit)),
        commit.author,
        iso_local(commit.at * 1000)
    )
}

fn version_line(cards: &Cards, deck: &rumu_cards::Deck, commit: &Commit, path: &str) -> String {
    let (op, _) = commit.op_path();
    format!(
        "{}  {op}  {}  {}  {}",
        commit.short,
        shown(cards, deck, path),
        commit.author,
        iso_local(commit.at * 1000)
    )
}

fn deck_err(e: impl std::fmt::Display) -> ToolError {
    ToolError::model(format!("history: {e}"))
}
