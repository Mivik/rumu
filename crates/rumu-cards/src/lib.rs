//! rumu-cards: decks, cards and their history.
//!
//! A card is a markdown file in a deck (`RUMU_HOME/decks/<id>/`), a bare git
//! repository whose `HEAD` tree is the deck's content; it is identified by the
//! deck id and its path within the deck. A session sees the decks mounted on
//! its branch under
//! `@rumu:/cards/<mount>/<path>`; its own deck is `/cards/self`. This crate
//! owns everything card-shaped:
//!
//! - [`card`] — the content model (frontmatter, rendering, revisions, the
//!   editable view);
//! - [`deck`] — deck identity, loading and git-backed history;
//! - [`cards`] — the mounted card space (links, dedup, writes);
//! - [`library`] — the user's view of every deck (the web Library page);
//! - [`usage`] — card use records derived from a branch path;
//! - [`usage_index`] — per-deck card use scores (`usage.json`);
//! - [`recall`] — BM25F over `keywords:`, path, description and body;
//! - [`search_text`] / [`list_text`] — the discovery outputs the workspace
//!   plugin's `search` / `list` tools render.
//!
//! The `search` tool itself, and memory (the catalog, recall, update
//! notices), live in rumu-plugins' memory and workspace plugins.

use std::collections::BTreeMap;

use rumu_core::{RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult, quote};
use serde_json::Value;

pub mod card;
pub mod cards;
pub mod catalog;
pub mod deck;
pub mod diff;
mod error;
pub mod git;
pub mod library;
pub mod recall;
pub mod usage;
pub mod usage_index;
pub mod yaml;

pub use card::{Card, CardInfo, valid_card_path};
pub use cards::Cards;
pub use deck::{Author, Deck, DeckCat, DeckIcon, DeckMeta, DeckMount, Decks, open_deck};
pub use error::{CardViewError, Error, Result};

/// Run a card search over the mounted card space: hits are BM25F-ranked like
/// recall (`keywords:`, path, description, body), any term matches. Each hit
/// is an `@rumu:/cards/<path>  "<summary>"` line, then an indented
/// `"<snippet>"` line around the first body match — never the full text.
pub fn search_text(cards: &Cards, query: &str, args: &Value) -> ToolResult<ToolOutput> {
    if query.trim().is_empty() {
        return Err(ToolError::model("search: empty query"));
    }
    let prefix = card::normalize_prefix(args.get("prefix").and_then(Value::as_str).unwrap_or(""));
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .max(1) as usize;

    let all: BTreeMap<String, Card> = cards
        .rankable()
        .map_err(|e| ToolError::Fatal(e.into()))?
        .into_iter()
        .filter(|(path, _)| card::under_prefix(path, &prefix))
        .collect();
    let hits = recall::search(&all, query, usize::MAX);
    if hits.is_empty() {
        return Ok(format!("no cards match `{query}`").into());
    }
    let terms = recall::tokens(query);
    let mut out = Vec::new();
    for hit in hits.iter().take(limit) {
        let mut block = format!("card {}", card_uri(&hit.card.path));
        if let Some(m) = hit.card.summary() {
            block.push_str(&format!("  {}", quote(&m, SUMMARY_CHARS)));
        }
        if let Some(s) = body_snippet(&hit.card.body, &terms) {
            block.push_str(&format!("\n  {}", quote(&s, usize::MAX)));
        }
        out.push(block);
    }
    let mut text = out.join("\n");
    if hits.len() > limit {
        text.push_str(&format!("\n(+{} more hits)", hits.len() - limit));
    }
    Ok(text.into())
}

/// Summary chars shown on a card-search hit line.
const SUMMARY_CHARS: usize = 80;

/// `@rumu:/cards/<path>` — the canonical address of a card in the mounted
/// space (`<path>` is `self/dev/x`).
pub fn card_uri(path: &str) -> String {
    format!("{RUMU_WORKSPACE}:/cards/{}", path.trim_start_matches('/'))
}

/// Render the `list @rumu:/cards[/<prefix>]` output: one
/// `@rumu:/cards/<path> — summary` line per card, never full text.
pub fn list_text(cards: &Cards, prefix: &str, limit: usize) -> ToolResult<ToolOutput> {
    let prefix = card::normalize_prefix(prefix);
    let all = cards.rankable().map_err(|e| ToolError::Fatal(e.into()))?;
    let cards: Vec<(String, Card)> = all
        .into_iter()
        .filter(|(path, _)| card::under_prefix(path, &prefix))
        .collect();
    if cards.is_empty() {
        return Ok(format!("no cards under `{prefix}`").into());
    }
    let mut lines = Vec::new();
    for (path, c) in cards.iter().take(limit) {
        lines.push(card_line(path, c));
    }
    let mut text = lines.join("\n");
    if cards.len() > limit {
        text.push_str(&format!(
            "\n(+{} more; narrow the prefix or use search)",
            cards.len() - limit
        ));
    }
    Ok(text.into())
}

/// One list line: `@rumu:/cards/<path> — summary`.
pub fn card_line(path: &str, c: &Card) -> String {
    let mut line = card_uri(path);
    if let Some(d) = c.summary() {
        line.push_str(&format!(" — {d}"));
    }
    line
}

/// A ≤160-char window around the first body hit of any term.
pub fn body_snippet(body: &str, terms: &[String]) -> Option<String> {
    let lower = body.to_lowercase();
    let pos = terms.iter().filter_map(|t| lower.find(t.as_str())).min()?;
    let start = pos.saturating_sub(60);
    // Snap to a char boundary and extend ~160 chars.
    let start = body.floor_char_boundary(start);
    let end = body.ceil_char_boundary((pos + 100).min(body.len()));
    let mut s: String = body[start..end].replace('\n', " ");
    if start > 0 {
        s.insert(0, '…');
    }
    if end < body.len() {
        s.push('…');
    }
    if s.len() > 170 {
        s.truncate(s.floor_char_boundary(165));
        s.push('…');
    }
    Some(s)
}

/// Up to 5 ids near `q`: longest common prefix wins, substring matches next.
/// Serves the near-miss suggestions of unknown-id card reads.
pub fn suggest_ids(ids: &[String], q: &str) -> Vec<String> {
    let ql = q.to_lowercase();
    let mut scored: Vec<(usize, String)> = ids
        .iter()
        .filter_map(|id| {
            let il = id.to_lowercase();
            let prefix = il
                .chars()
                .zip(ql.chars())
                .take_while(|(a, b)| a == b)
                .count();
            let sub = il.contains(&ql);
            (prefix > 0 || sub).then(|| (prefix * 2 + usize::from(sub), id.clone()))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(5).map(|(_, id)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::CardMount;

    fn cards_home() -> (tempfile::TempDir, Cards) {
        let dir = tempfile::tempdir().unwrap();
        let decks = Decks::load(dir.path()).unwrap();
        let deck = decks
            .upsert(DeckMeta {
                id: String::new(),
                name: "main".into(),
                assistant: false,
                project: false,
                private: false,
                mounts: Vec::new(),
                allow_ask: Vec::new(),
                model: None,
                mcp: Default::default(),
                skills: Vec::new(),
                icon: None,
                cat: None,
            })
            .unwrap();
        for (path, text) in [
            (
                "x",
                "---\n---\nthe kernel crate implements the agent loop\nSECRET SECOND LINE\n",
            ),
            ("y", "---\ndescription: why\n---\nnothing here\n"),
            (
                "t",
                "---\ndescription: about the loop\nkeywords: [agent loop]\n---\nfiller\n",
            ),
        ] {
            deck.write_file(path, text, &Author::user()).unwrap();
        }
        let cards = Cards::new(
            &decks,
            &[CardMount {
                deck: deck.id().to_owned(),
                at: "/cards/self".into(),
            }],
        );
        (dir, cards)
    }

    #[test]
    fn search_ranks_snippets_and_scopes() {
        let (_dir, store) = cards_home();

        // Body hit ranks below keyword hits; addresses, snippets, never full text.
        let out = search_text(&store, "agent loop", &serde_json::json!({})).unwrap();
        let first = out
            .content
            .text
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        assert_eq!(
            first, "card @rumu:/cards/self/t  \"about the loop\"",
            "keyword hit ranks first: {}",
            out.content.text
        );
        assert!(
            out.content.text.contains("@rumu:/cards/self/x"),
            "{}",
            out.content.text
        );
        assert!(
            out.content.text.contains(
                "card @rumu:/cards/self/x  \"the kernel crate implements the agent loop\"\n  \"the kernel"
            ),
            "summary line, then the quoted snippet: {}",
            out.content.text
        );
        assert!(
            !out.content.text.contains("filler"),
            "no unrelated body text: {}",
            out.content.text
        );

        // Terms are ORed: a query with one unknown term still matches.
        let out = search_text(&store, "agent loop missing-word", &serde_json::json!({})).unwrap();
        assert!(
            out.content.text.contains("@rumu:/cards/self/"),
            "{}",
            out.content.text
        );
        assert!(
            !out.content.text.starts_with("no cards match"),
            "{}",
            out.content.text
        );
        let out = search_text(&store, "zebra", &serde_json::json!({})).unwrap();
        assert!(!out.is_error);
        assert!(
            out.content.text.starts_with("no cards match"),
            "{}",
            out.content.text
        );

        // Prefix scoping and limits.
        let out = list_text(&store, "self/x", 100).unwrap();
        assert!(
            out.content
                .text
                .contains("@rumu:/cards/self/x — the kernel crate implements the agent loop"),
            "{}",
            out.content.text
        );
        assert!(
            !out.content.text.contains("@rumu:/cards/self/y"),
            "{}",
            out.content.text
        );
        assert!(
            !out.content.text.contains("SECRET SECOND LINE"),
            "list must not leak whole bodies: {}",
            out.content.text
        );
        let out = search_text(&store, "agent", &serde_json::json!({ "limit": 1 })).unwrap();
        assert!(
            out.content.text.contains("(+1 more hits)"),
            "{}",
            out.content.text
        );
    }
}
