//! The layered card catalog.
//!
//! Every branch sees one root catalog on its first `llm.pre`: all cards when
//! the library is small, otherwise top-level cards plus the most recent /
//! most used deep cards, with the rest folded into directory summary lines.
//! Subsequent `llm.pre`s progressively unfold a folded directory once the
//! branch touches a card under it, and a workspace's cards once the branch
//! switches to that workspace.
//!
//! Frecency = file freshness (`mtime`, 7-day half-life) plus active read /
//! write use across sessions (30-day half-life; auto-import notes do not
//! count, so imports cannot inflate their own score).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use rumu_core::{Node, NodeBody, RUMU_WORKSPACE, Structured, Timestamp, WorkspaceUse};

use crate::Card;
use crate::card::under_prefix;
use crate::usage::{self, CardOp, CardUse};

/// One card entry in a structured catalog note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CatalogEntry {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A folded directory summary in the root catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FoldedCatalogDir {
    pub prefix: String,
    /// Card names relative to `prefix`.
    pub names: Vec<String>,
}

/// What a catalog note covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum CatalogScope {
    Root,
    Directory { prefix: String },
    Workspace { name: String },
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
    const V: u32 = 2;
}

impl CatalogNote {
    /// The model-facing Markdown rendering. Cards are markdown links; folded
    /// directories stay inline code because they are prefixes, not cards.
    pub fn markdown(&self) -> String {
        let mut lines = Vec::new();
        match &self.scope {
            CatalogScope::Root => lines.push(format!(
                "**Card catalog** (read `{RUMU_WORKSPACE}:/card/<id>` when relevant):"
            )),
            CatalogScope::Directory { prefix } => lines.push(format!(
                "**Cards under `{RUMU_WORKSPACE}:/card/{prefix}/`:**"
            )),
            CatalogScope::Workspace { name } => {
                lines.push(format!("**Cards related to workspace `{name}`:**"))
            }
        }
        for e in &self.entries {
            let mut line = format!(
                "- [{}]({RUMU_WORKSPACE}:/card/{})",
                escape_link_text(&e.title),
                e.id
            );
            if let Some(s) = &e.summary {
                line.push_str(&format!(" — {s}"));
            }
            lines.push(line);
        }
        if !self.folded.is_empty() {
            lines.push(format!(
                "**Folded directories** (use `list {RUMU_WORKSPACE}:/card/<dir>` or `search` \
                     to inspect):"
            ));
            for f in &self.folded {
                let shown = f.names.len().min(5);
                let mut names = f.names[..shown].join(", ");
                if f.names.len() > shown {
                    names.push('…');
                }
                lines.push(format!(
                    "- `{}/` ({} more cards: {})",
                    f.prefix,
                    f.names.len(),
                    names
                ));
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

/// The root catalog expands at most this many cards; the rest fold by dir.
pub const ROOT_CATALOG_BUDGET: usize = 40;
/// Weight of the freshness term (a fresh card outranks one cold read).
pub const FRESH_WEIGHT: f64 = 3.0;
/// Half-life of the freshness term: 7 days in ms.
pub const FRESH_HALF_LIFE_MS: f64 = 7.0 * 86_400_000.0;
/// Half-life of active use: 30 days in ms.
pub const USE_HALF_LIFE_MS: f64 = 30.0 * 86_400_000.0;

/// Active-use history extracted from one session's nodes.
#[derive(Debug, Clone, Default)]
pub struct SessionTouch {
    pub workspaces: BTreeSet<String>,
    /// card_id -> (latest_at_ms, max_weight) within this session.
    pub card_uses: BTreeMap<String, (Timestamp, f64)>,
}

/// Collect the workspaces a session switched to and the cards it actively
/// read or wrote. Auto-import notes (`memory.import`) are deliberately
/// ignored.
pub fn extract_session_touch(nodes: &[Node]) -> SessionTouch {
    let ok_calls = usage::ok_calls(nodes);

    let mut touch = SessionTouch::default();
    for n in nodes {
        let NodeBody::Note { content, .. } = &n.body else {
            continue;
        };
        if let Some(ws) = content.decode::<WorkspaceUse>() {
            touch.workspaces.insert(ws.name);
            continue;
        }
        let Some(u) = content.decode::<CardUse>() else {
            continue;
        };
        // A use whose call never produced a successful result is stale.
        if let Some(call_id) = u.call_id.as_deref()
            && !ok_calls.contains(call_id)
        {
            continue;
        }
        let weight = match u.op {
            CardOp::Read => 1.0,
            CardOp::Write | CardOp::Edit => 2.0,
        };
        touch
            .card_uses
            .entry(u.id)
            .and_modify(|(at, w)| {
                *at = (*at).max(n.at);
                *w = w.max(weight);
            })
            .or_insert((n.at, weight));
    }
    touch
}

fn exp2_decay(delta_ms: i64, half_life: f64) -> f64 {
    2.0_f64.powf(-(delta_ms.max(0) as f64) / half_life)
}

/// Frecency of one card: freshness from its mtime plus active use in any of
/// `sessions`, both decaying exponentially.
pub fn score_card(
    id: &str,
    mtime: Option<Timestamp>,
    sessions: &[SessionTouch],
    now_ms: Timestamp,
) -> f64 {
    let fresh = mtime
        .map(|m| FRESH_WEIGHT * exp2_decay(now_ms - m, FRESH_HALF_LIFE_MS))
        .unwrap_or(0.0);
    let mut used = 0.0;
    for touch in sessions {
        if let Some((at, weight)) = touch.card_uses.get(id) {
            used += weight * exp2_decay(now_ms - *at, USE_HALF_LIFE_MS);
        }
    }
    fresh + used
}

/// Use score of one card restricted to sessions that touched `ws`.
pub fn score_card_for_workspace(
    id: &str,
    ws: &str,
    sessions: &[SessionTouch],
    now_ms: Timestamp,
) -> f64 {
    sessions
        .iter()
        .filter(|t| t.workspaces.contains(ws))
        .filter_map(|t| t.card_uses.get(id))
        .map(|(at, weight)| weight * exp2_decay(now_ms - *at, USE_HALF_LIFE_MS))
        .sum()
}

/// A folded directory: its prefix plus the unexpanded card ids under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedDir {
    pub prefix: String,
    pub cards: Vec<String>,
}

/// The root catalog's selection for one library snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootCatalog {
    pub expanded: BTreeSet<String>,
    pub folded: Vec<FoldedDir>,
}

impl RootCatalog {
    /// Every id not expanded (i.e. folded into a directory summary).
    pub fn unexpanded(&self) -> BTreeSet<String> {
        self.folded
            .iter()
            .flat_map(|f| f.cards.iter().cloned())
            .collect()
    }
}

/// Split `budget` between protected top-level cards and the highest-scoring
/// deep cards; the rest fold into directory summaries.
pub fn plan_root_catalog(
    cards: &BTreeMap<String, Card>,
    scores: &BTreeMap<String, f64>,
    budget: usize,
) -> RootCatalog {
    if cards.is_empty() {
        return RootCatalog::default();
    }
    if cards.len() <= budget {
        return RootCatalog {
            expanded: cards.keys().cloned().collect(),
            folded: Vec::new(),
        };
    }

    let mut expanded: BTreeSet<String> = cards
        .keys()
        .filter(|id| !id.contains('/'))
        .cloned()
        .collect();

    let mut deep: Vec<&String> = cards.keys().filter(|id| id.contains('/')).collect();
    deep.sort_by(|a, b| {
        scores
            .get(*b)
            .copied()
            .unwrap_or(0.0)
            .total_cmp(&scores.get(*a).copied().unwrap_or(0.0))
            .then_with(|| a.cmp(b))
    });
    let room = budget.saturating_sub(expanded.len());
    expanded.extend(deep.into_iter().take(room).cloned());

    // Group the remaining deep cards by top dir, narrowed to the longest
    // common directory prefix when the whole group shares one.
    let mut by_top: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in cards.keys() {
        if expanded.contains(id) || !id.contains('/') {
            continue;
        }
        let top = id.split('/').next().unwrap().to_owned();
        by_top.entry(top).or_default().push(id.clone());
    }
    let mut folded: Vec<FoldedDir> = by_top
        .into_values()
        .map(|ids| FoldedDir {
            prefix: common_dir_prefix(&ids),
            cards: ids,
        })
        .collect();
    folded.sort_by(|a, b| a.prefix.cmp(&b.prefix));
    for f in &mut folded {
        f.cards.sort();
    }
    RootCatalog { expanded, folded }
}

/// The deepest directory prefix shared by every id (one segment is always
/// shared, since grouping is per top-level dir).
fn common_dir_prefix(ids: &[String]) -> String {
    let dirs: Vec<Vec<&str>> = ids
        .iter()
        .map(|id| {
            let segs: Vec<&str> = id.split('/').collect();
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

fn sorted_by_score<'a>(
    ids: impl Iterator<Item = &'a String>,
    scores: &BTreeMap<String, f64>,
) -> Vec<&'a String> {
    let mut ids: Vec<&String> = ids.collect();
    ids.sort_by(|a, b| {
        scores
            .get(*b)
            .copied()
            .unwrap_or(0.0)
            .total_cmp(&scores.get(*a).copied().unwrap_or(0.0))
            .then_with(|| a.cmp(b))
    });
    ids
}

/// The structured root catalog.
pub fn root_catalog_note(cards: &BTreeMap<String, Card>, plan: &RootCatalog) -> CatalogNote {
    let entries = plan
        .expanded
        .iter()
        .filter_map(|id| cards.get(id).map(|c| entry(id, c)))
        .collect();
    let folded = plan
        .folded
        .iter()
        .map(|f| FoldedCatalogDir {
            prefix: f.prefix.clone(),
            names: f
                .cards
                .iter()
                .map(|id| relative_name(id, &f.prefix).to_owned())
                .collect(),
        })
        .collect();
    CatalogNote {
        scope: CatalogScope::Root,
        entries,
        folded,
    }
}

fn entry(id: &str, c: &Card) -> CatalogEntry {
    CatalogEntry {
        id: id.to_owned(),
        title: c.title.clone().unwrap_or_else(|| id.to_owned()),
        summary: c.summary(),
    }
}

fn entries_for<'a>(
    ids: impl IntoIterator<Item = &'a String>,
    cards: &BTreeMap<String, Card>,
) -> Vec<CatalogEntry> {
    ids.into_iter()
        .filter_map(|id| cards.get(id).map(|c| entry(id, c)))
        .collect()
}

/// A folded card's name relative to its group prefix (`dev/rumu/a` under
/// `dev/rumu` → `a`).
fn relative_name<'a>(id: &'a str, prefix: &str) -> &'a str {
    id.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('/'))
        .filter(|r| !r.is_empty())
        .unwrap_or(id)
}

/// A progressive-unfold note and its structured catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfoldNote {
    pub catalog: CatalogNote,
}

/// Everything the root/progressive catalog planning needs for one turn.
pub struct CatalogInput<'a> {
    pub cards: &'a BTreeMap<String, Card>,
    pub scores: &'a BTreeMap<String, f64>,
    pub path_nodes: &'a [Node],
    /// Catalog scopes already unfolded on this path (from the index).
    pub unfolded: &'a [CatalogScope],
    pub default_workspace: Option<&'a str>,
    pub sessions: &'a [SessionTouch],
    pub now_ms: Timestamp,
}

/// Split the already-unfolded scopes by kind.
fn unfolded_scopes(scopes: &[CatalogScope]) -> (bool, BTreeSet<String>, BTreeSet<String>) {
    let mut root = false;
    let mut dirs = BTreeSet::new();
    let mut workspaces = BTreeSet::new();
    for scope in scopes {
        match scope {
            CatalogScope::Root => root = true,
            CatalogScope::Directory { prefix } => {
                dirs.insert(prefix.clone());
            }
            CatalogScope::Workspace { name } => {
                workspaces.insert(name.clone());
            }
        }
    }
    (root, dirs, workspaces)
}

/// Plan the notes to inject this turn: the root catalog once per branch, then
/// at most one note per newly touched folded directory and one per workspace.
pub fn plan_unfold_notes(input: &CatalogInput<'_>) -> Vec<UnfoldNote> {
    if input.cards.is_empty() {
        return Vec::new();
    }
    let plan = plan_root_catalog(input.cards, input.scores, ROOT_CATALOG_BUDGET);
    let (root_seen, dirs_seen, workspaces_seen) = unfolded_scopes(input.unfolded);
    let mut notes = Vec::new();

    if !root_seen {
        notes.push(UnfoldNote {
            catalog: root_catalog_note(input.cards, &plan),
        });
    }

    // Progressive unfolding only exists while something is folded.
    if plan.folded.is_empty() {
        return notes;
    }

    // Directory unfolding: a card used on this path lives under a folded
    // directory → show that directory's remaining cards.
    let used = usage::uses_on_path(input.path_nodes);
    for f in &plan.folded {
        let touched = used.keys().any(|id| under_prefix(id, &f.prefix));
        if !touched || dirs_seen.contains(&f.prefix) {
            continue;
        }
        let entries = entries_for(
            sorted_by_score(f.cards.iter(), input.scores)
                .into_iter()
                .take(20),
            input.cards,
        );
        notes.push(UnfoldNote {
            catalog: CatalogNote {
                scope: CatalogScope::Directory {
                    prefix: f.prefix.clone(),
                },
                entries,
                folded: Vec::new(),
            },
        });
    }

    // Workspace unfolding: a real workspace switch pulls in its cards.
    if let Some(ws) = input.default_workspace
        && ws != RUMU_WORKSPACE
        && !workspaces_seen.contains(ws)
    {
        let dev = format!("dev/{ws}");
        let unexpanded = plan.unexpanded();
        let matching: Vec<&String> = unexpanded
            .iter()
            .filter(|id| {
                under_prefix(id, ws)
                    || under_prefix(id, &dev)
                    || score_card_for_workspace(id, ws, input.sessions, input.now_ms) > 0.0
            })
            .collect();
        if !matching.is_empty() {
            let entries = entries_for(
                sorted_by_score(matching.into_iter(), input.scores)
                    .into_iter()
                    .take(20),
                input.cards,
            );
            notes.push(UnfoldNote {
                catalog: CatalogNote {
                    scope: CatalogScope::Workspace {
                        name: ws.to_owned(),
                    },
                    entries,
                    folded: Vec::new(),
                },
            });
        }
    }

    notes
}

#[cfg(test)]
mod tests;
