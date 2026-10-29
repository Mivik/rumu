//! Card `scope:`: the workspaces and files a card is related to — a string
//! or a list of strings in the frontmatter. An entry is a workspace name
//! (`proj`, the whole workspace) or a workspace-relative glob
//! (`proj:crates/rumu-kernel/**`). Entries naming another workspace, or
//! carrying an invalid glob, simply never match: cards are portable across
//! machines with different presets.

use std::collections::BTreeMap;

use globset::Glob;
use serde_json::Value;

use crate::Card;

/// The cards whose scope covers a touch of `workspace` at the
/// workspace-relative `paths` (empty for a workspace-only touch), most
/// specific first, then by id.
pub fn related<'a>(
    cards: &'a BTreeMap<String, Card>,
    workspace: &str,
    paths: &[&str],
) -> Vec<&'a Card> {
    let mut hits: Vec<(usize, &Card)> = cards
        .values()
        .filter_map(|c| specificity(c, workspace, paths).map(|s| (s, c)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    hits.into_iter().map(|(_, c)| c).collect()
}

/// How specifically `card` covers the touch: the literal path segments of
/// its best matching entry (0 for a whole-workspace entry), `None` when no
/// entry matches.
fn specificity(card: &Card, workspace: &str, paths: &[&str]) -> Option<usize> {
    let entries: Vec<&str> = match card.meta.get("scope") {
        Some(Value::String(s)) => vec![s],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    entries
        .into_iter()
        .filter_map(|entry| match entry.split_once(':') {
            None => (entry == workspace).then_some(0),
            Some((ws, pattern)) if ws == workspace => {
                let glob = Glob::new(pattern).ok()?.compile_matcher();
                // `list` touches a directory, which `dir/**` only matches
                // with a trailing slash.
                paths
                    .iter()
                    .any(|p| glob.is_match(p) || glob.is_match(format!("{p}/")))
                    .then(|| literal_segments(pattern))
            }
            Some(_) => None,
        })
        .max()
}

/// The number of literal path segments before the first glob metacharacter:
/// `crates/rumu-kernel/**` → 2, `src/main.rs` → 2, `**` → 0.
fn literal_segments(pattern: &str) -> usize {
    pattern
        .split('/')
        .filter(|s| !s.is_empty())
        .take_while(|s| !s.contains(['*', '?', '[', '{']))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_card;

    fn cards(entries: &[(&str, &str)]) -> BTreeMap<String, Card> {
        entries
            .iter()
            .map(|(id, scope)| {
                let text = format!("---\nscope: {scope}\n---\nbody of {id}\n");
                ((*id).to_owned(), parse_card(id, &text))
            })
            .collect()
    }

    fn ids(cards: Vec<&Card>) -> Vec<&str> {
        cards.into_iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn most_specific_first() {
        let cards = cards(&[
            ("whole", "[proj]"),
            ("tree", "[proj:crates/rumu-kernel/**]"),
            ("file", "[proj:crates/rumu-kernel/src/dispatch.rs]"),
            ("wide", "[proj:**]"),
            ("other", "[app, 'app:crates/**']"),
        ]);
        let touch = ["crates/rumu-kernel/src/dispatch.rs"];
        assert_eq!(
            ids(related(&cards, "proj", &touch)),
            ["file", "tree", "whole", "wide"]
        );
        // A workspace-only touch matches whole-workspace entries only.
        assert_eq!(ids(related(&cards, "proj", &[])), ["whole"]);
        // A file in another tree.
        assert_eq!(
            ids(related(&cards, "proj", &["web/app.vue"])),
            ["whole", "wide"]
        );
    }

    #[test]
    fn a_listed_directory_matches_its_tree_glob() {
        // A single string is one entry.
        let cards = cards(&[("tree", "proj:crates/rumu-kernel/**")]);
        assert_eq!(
            ids(related(&cards, "proj", &["crates/rumu-kernel"])),
            ["tree"]
        );
    }

    #[test]
    fn foreign_and_invalid_entries_never_match() {
        let cards = cards(&[("bad", "['proj:crates/[x']"), ("ghost", "['ghost:**']")]);
        assert!(related(&cards, "proj", &["crates/x"]).is_empty());
    }
}
