//! Per-deck card catalogs.
//!
//! Every mounted deck has its own catalog note. It lists the deck's
//! [`CATALOG_BUDGET`] highest-use cards in path order, as links with their
//! summary, and folds the rest into one line per directory. Use comes from the
//! deck's usage index (`usage.json`), so a card's score survives moves within
//! the deck; ties go by path. Directories are ordered by total score and names
//! by score.
//!
//! A catalog is frozen once written: later changes to the deck show up through
//! update notices, not by rewriting the catalog.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use rumu_core::{RUMU_WORKSPACE, Structured, Timestamp};

use crate::card::{Card, parse_card};
use crate::deck::{Deck, Decks};
use crate::usage_index;

/// Cards listed in full; the rest fold by directory.
pub const CATALOG_BUDGET: usize = 30;

/// One card entry in a structured catalog note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CatalogEntry {
    /// The card's path under its mount point (`self/dev/x`).
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// A link whose target is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangling: Option<String>,
    /// The last author when it is not the current assistant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}

/// A folded directory summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FoldedCatalogDir {
    /// The directory prefix; empty for cards at the deck root.
    pub prefix: String,
    /// Card names relative to `prefix`, by score.
    pub names: Vec<String>,
}

/// Which deck a catalog note covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CatalogScope {
    /// The deck's id.
    pub deck: String,
    /// The deck's mount point (`/cards/self`).
    pub at: String,
}

/// Structured card index carried in a [`rumu_core::NodeBody::Note`]'s
/// `content.data` under the `memory.catalog` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CatalogNote {
    pub scope: CatalogScope,
    pub entries: Vec<CatalogEntry>,
    pub folded: Vec<FoldedCatalogDir>,
}

impl Structured for CatalogNote {
    const TAG: &'static str = "memory.catalog";
    const V: u32 = 3;
}

impl CatalogNote {
    /// The model-facing Markdown rendering. Cards are markdown links; folded
    /// directories stay inline code because they are prefixes, not cards.
    pub fn markdown(&self) -> String {
        if self.entries.is_empty() && self.folded.is_empty() {
            return String::new();
        }
        let mut lines = vec![format!(
            "**Card catalog** (`{RUMU_WORKSPACE}:{}`):",
            self.scope.at
        )];
        for e in &self.entries {
            let mut line = format!(
                "- [{}]({RUMU_WORKSPACE}:/cards/{})",
                escape_link_text(&e.id),
                e.id
            );
            if let Some(s) = &e.summary {
                line.push_str(&format!(" — {s}"));
            }
            if let Some(d) = &e.dangling {
                line.push_str(&format!(" — (dangling link to `{d}`)"));
            }
            if let Some(a) = &e.author {
                line.push_str(&format!(" — edited by {a}"));
            }
            lines.push(line);
        }
        if !self.folded.is_empty() {
            lines.push(format!(
                "**Folded directories** (use `list {RUMU_WORKSPACE}:/cards/<dir>` or `search` \
                     to inspect):"
            ));
            for f in &self.folded {
                let shown = f.names.len().min(5);
                let mut names = f.names[..shown].join(", ");
                if f.names.len() > shown {
                    names.push('…');
                }
                let dir = if f.prefix.is_empty() {
                    "(deck root)".to_owned()
                } else {
                    format!("`{}/`", f.prefix)
                };
                lines.push(format!("- {dir} ({} more cards: {names})", f.names.len()));
            }
        }
        lines.join("\n")
    }
}

fn escape_link_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// One candidate row while planning a deck's catalog.
struct Row {
    /// Deck-local path.
    path: String,
    /// Public path under the mount point (`self/dev/x`).
    id: String,
    summary: Option<String>,
    dangling: Option<String>,
    author: Option<String>,
    score: f64,
}

/// Render the catalog of one mounted deck at `at` (its mount point). Usage
/// scores come from the deck's own index; a link row takes its target's.
/// `current` is the session's assistant deck id: its own edits are not named.
pub fn deck_catalog(
    decks: &Decks,
    deck: &Deck,
    at: &str,
    current: &str,
    now_ms: Timestamp,
) -> crate::Result<CatalogNote> {
    let point = at.strip_prefix("/cards/").unwrap_or(at);
    let mut score_cache: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut author_cache: BTreeMap<String, BTreeMap<String, (String, String)>> = BTreeMap::new();
    let mut rows = Vec::new();

    for path in deck.walk()? {
        let text = match deck.read(&path) {
            Ok(Some(text)) => text,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(deck = %deck.name, path = %path, error = %e, "skipping an unreadable card");
                continue;
            }
        };
        let raw = parse_card(&path, &text);
        // Pinned cards are loaded in full next to the catalog.
        if raw.pinned() && raw.link().is_none() {
            continue;
        }
        let mut content_deck = deck.id().to_owned();
        let mut content_path = path.clone();
        let mut key = raw.id().map(str::to_owned);
        let mut summary = raw.summary();
        let mut dangling = None;
        if let Some(stored) = raw.link() {
            match resolve(decks, stored) {
                Some((target_deck, target_path, target)) => {
                    key = target.id().map(str::to_owned);
                    summary = target.summary();
                    content_deck = target_deck;
                    content_path = target_path;
                }
                None => dangling = Some(stored.to_owned()),
            }
        }
        let score = key
            .and_then(|key| {
                scores_for(&mut score_cache, decks, &content_deck, now_ms)
                    .get(&key)
                    .copied()
            })
            .unwrap_or(0.0);
        let author = authors_for(&mut author_cache, decks, &content_deck)
            .get(&content_path)
            .filter(|(id, _)| id != current)
            .map(|(_, name)| name.clone());
        rows.push(Row {
            id: format!("{point}/{path}"),
            path,
            summary,
            dangling,
            author,
            score,
        });
    }

    rows.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.path.cmp(&b.path))
    });
    let split = rows.len().min(CATALOG_BUDGET);
    let folded_rows = rows.split_off(split);
    let mut entries: Vec<CatalogEntry> = rows
        .into_iter()
        .map(|r| CatalogEntry {
            id: r.id,
            summary: r.summary,
            dangling: r.dangling,
            author: r.author,
        })
        .collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));

    // Fold by top-level directory; cards at the root form one group.
    let mut by_top: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    for row in folded_rows {
        let top = row
            .path
            .split_once('/')
            .map(|(top, _)| top.to_owned())
            .unwrap_or_default();
        by_top.entry(top).or_default().push(row);
    }
    let mut groups: Vec<(f64, FoldedCatalogDir)> = Vec::new();
    for (_, mut group) in by_top {
        group.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.path.cmp(&b.path))
        });
        let paths: Vec<String> = group.iter().map(|r| r.path.clone()).collect();
        let prefix = common_dir_prefix(&paths);
        let total = group.iter().map(|r| r.score).sum();
        let names = group
            .iter()
            .map(|r| relative_name(&r.path, &prefix).to_owned())
            .collect();
        groups.push((total, FoldedCatalogDir { prefix, names }));
    }
    groups.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.prefix.cmp(&b.1.prefix))
    });

    Ok(CatalogNote {
        scope: CatalogScope {
            deck: deck.id().to_owned(),
            at: at.to_owned(),
        },
        entries,
        folded: groups.into_iter().map(|(_, f)| f).collect(),
    })
}

/// A stored link (`<deck-id>:<path>`) resolved to its content.
fn resolve(decks: &Decks, stored: &str) -> Option<(String, String, Card)> {
    let (deck_id, path) = stored.split_once(':')?;
    let deck = decks.get(deck_id)?;
    let text = deck.read(path).ok().flatten()?;
    Some((deck_id.to_owned(), path.to_owned(), parse_card(path, &text)))
}

/// Effective scores per deck, computed once.
fn scores_for<'a>(
    cache: &'a mut BTreeMap<String, BTreeMap<String, f64>>,
    decks: &Decks,
    deck_id: &str,
    now_ms: Timestamp,
) -> &'a BTreeMap<String, f64> {
    cache.entry(deck_id.to_owned()).or_insert_with(|| {
        decks
            .get(deck_id)
            .map(|deck| usage_index::scores(&deck.dir, now_ms))
            .unwrap_or_default()
    })
}

/// Last author per card path for a deck, from its newest 500 commits: the
/// author's id and display name (a deck's current name, not the one it had
/// when it committed).
fn authors_for<'a>(
    cache: &'a mut BTreeMap<String, BTreeMap<String, (String, String)>>,
    decks: &Decks,
    deck_id: &str,
) -> &'a BTreeMap<String, (String, String)> {
    cache.entry(deck_id.to_owned()).or_insert_with(|| {
        let Some(deck) = decks.get(deck_id) else {
            return BTreeMap::new();
        };
        let mut out = BTreeMap::new();
        for commit in deck.commits(500).unwrap_or_default() {
            let id = commit.author_id();
            let name = decks
                .get(id)
                .map_or_else(|| commit.author.clone(), |d| d.meta.name);
            for path in &commit.changed {
                out.entry(path.clone())
                    .or_insert_with(|| (id.to_owned(), name.clone()));
            }
        }
        out
    })
}

/// The deepest directory prefix shared by every path (one segment is always
/// shared, since grouping is per top-level dir).
fn common_dir_prefix(paths: &[String]) -> String {
    let dirs: Vec<Vec<&str>> = paths
        .iter()
        .map(|path| {
            let segs: Vec<&str> = path.split('/').collect();
            segs[..segs.len().saturating_sub(1)].to_vec()
        })
        .collect();
    let Some(first) = dirs.first() else {
        return String::new();
    };
    let mut len = first.len();
    for d in &dirs[1..] {
        len = len.min(d.len());
        while len > 0 && first[..len] != d[..len] {
            len -= 1;
        }
    }
    first[..len].join("/")
}

/// A folded card's name relative to its group prefix (`dev/rumu/a` under
/// `dev/rumu` → `a`).
fn relative_name<'a>(path: &'a str, prefix: &str) -> &'a str {
    path.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('/'))
        .filter(|r| !r.is_empty())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests;
