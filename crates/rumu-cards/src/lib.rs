//! rumu-cards: the card library capability.
//!
//! A card is a markdown knowledge file addressed by a path id
//! (`dev/rumu/kernel`) in the home library under `RUMU_HOME/cards`
//! (`@rumu:/cards/<id>`). This crate owns everything card-shaped:
//!
//! - [`card`] — the content model (frontmatter, rendering, revisions, the
//!   editable view, id validation);
//! - [`store`] — the storage abstraction behind a library (home = local fs
//!   with symlink/escape guards);
//! - [`library`] — card loading/writing over a store;
//! - [`usage`] — card use records derived from a branch path;
//! - [`scope`] — which cards a card's frontmatter `scope:` relates to a
//!   workspace / file touch;
//! - [`recall`] — BM25F over `keywords:`, id, description and body: the
//!   cards whose text best matches a prompt (recall) or a search query;
//! - [`search_text`] / [`list_text`] — the discovery outputs the workspace
//!   plugin's `search` / `list` tools render (addresses are `@rumu:/cards/<id>`).
//!
//! The `search` tool itself, and memory (the catalog, scope cards, update
//! notices), live in
//! rumu-plugins' memory and workspace plugins, which consume this crate:
//! cards are the content, memory is one strategy for surfacing them.

use std::collections::BTreeMap;

use rumu_core::{RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult};
use serde_json::Value;

pub mod card;
pub mod catalog;
mod error;
pub mod library;
pub mod recall;
pub mod scope;
pub mod store;
pub mod usage;
pub mod yaml;

pub use card::{Card, CardInfo, valid_card_id};
pub use error::{CardViewError, Error, Result};
pub use store::FsCardStore;

/// Run a card search over the home library and render its output text: hits
/// are BM25F-ranked like recall (`keywords:`, id, description, body), any
/// term matches. Each hit is an `@rumu:/cards/<id>  "<summary>"` line, then an
/// indented `"<snippet>"` line around the first body match — never the full
/// text. Summary and snippet are JSON string literals (see [`json_quote`]).
pub fn search_text(store: &FsCardStore, query: &str, args: &Value) -> ToolResult<ToolOutput> {
    if query.trim().is_empty() {
        return Err(ToolError::model("search: empty query"));
    }
    let prefix = card::normalize_prefix(args.get("prefix").and_then(Value::as_str).unwrap_or(""));
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .max(1) as usize;

    let cards: BTreeMap<String, Card> = library::cards_under(store, &prefix)?.into_iter().collect();
    // An unbounded limit keeps the total hit count for the `(+N more)` tail.
    let hits = recall::search(&cards, query, usize::MAX);
    if hits.is_empty() {
        return Ok(format!("no cards match `{query}`").into());
    }
    let terms = recall::tokens(query);
    let mut out = Vec::new();
    for hit in hits.iter().take(limit) {
        let mut block = card_uri(&hit.card.id);
        if let Some(m) = hit.card.summary() {
            block.push_str(&format!("  {}", json_quote(&m, SUMMARY_CHARS)));
        }
        if let Some(s) = body_snippet(&hit.card.body, &terms) {
            block.push_str(&format!("\n  {}", json_quote(&s, usize::MAX)));
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

/// `s` cut to `max` chars, as a JSON string literal; a cut appends `…`
/// outside the quotes so the literal stays the text's exact prefix. The
/// quoting of titles, summaries and snippets in `search` output.
pub fn json_quote(s: &str, max: usize) -> String {
    let cut: String = s.chars().take(max).collect();
    let mut q = serde_json::to_string(&cut).unwrap_or_default();
    if cut.len() < s.len() {
        q.push('…');
    }
    q
}

/// `@rumu:/cards/<id>` — the canonical address of a home card.
pub fn card_uri(id: &str) -> String {
    format!("{RUMU_WORKSPACE}:/cards/{id}")
}

/// Render the `list @rumu:/cards[/<prefix>]` output: one
/// `@rumu:/cards/<id> — summary` line per card, never full text.
pub fn list_text(store: &FsCardStore, prefix: &str, limit: usize) -> ToolResult<ToolOutput> {
    let prefix = card::normalize_prefix(prefix);
    let cards = library::cards_under(store, &prefix)?;
    if cards.is_empty() {
        return Ok(format!("no cards under `{prefix}`").into());
    }
    let mut lines = Vec::new();
    for (id, c) in cards.iter().take(limit) {
        lines.push(card_line(id, c));
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

/// One list line: `@rumu:/cards/<id> — summary`.
pub fn card_line(id: &str, c: &Card) -> String {
    let mut line = card_uri(id);
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
    use store::FsCardStore;

    #[test]
    fn search_ranks_snippets_and_scopes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("cards")).unwrap();
        std::fs::write(
            dir.path().join("cards/x.md"),
            "---\n---\nthe kernel crate implements the agent loop\nSECRET SECOND LINE\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("cards/y.md"),
            "---\ndescription: why\n---\nnothing here\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("cards/t.md"),
            "---\ndescription: about the loop\nkeywords: [agent loop]\n---\nfiller\n",
        )
        .unwrap();
        let store = FsCardStore::home(dir.path());

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
            first, "@rumu:/cards/t  \"about the loop\"",
            "keyword hit ranks first: {}",
            out.content.text
        );
        assert!(
            out.content.text.contains("@rumu:/cards/x"),
            "{}",
            out.content.text
        );
        assert!(
            out.content.text.contains(
                "@rumu:/cards/x  \"the kernel crate implements the agent loop\"\n  \"the kernel"
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
            out.content.text.contains("@rumu:/cards/"),
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
        let out = list_text(&store, "x", 100).unwrap();
        assert!(
            out.content
                .text
                .contains("@rumu:/cards/x — the kernel crate implements the agent loop"),
            "{}",
            out.content.text
        );
        assert!(
            !out.content.text.contains("@rumu:/cards/y"),
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

    #[test]
    fn json_quote_escapes_and_cuts_outside_the_quotes() {
        assert_eq!(json_quote("a \"b\"\nc", 10), r#""a \"b\"\nc""#);
        assert_eq!(json_quote("什么都可以", 3), "\"什么都\"…");
        assert_eq!(json_quote("abc", 3), "\"abc\"");
    }
}
