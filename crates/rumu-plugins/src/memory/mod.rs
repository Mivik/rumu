//! The memory plugin.
//!
//! Memory is the capability of keeping, finding and using **cards**: markdown
//! knowledge files addressed by path ids (`dev/rumu/kernel`) in the home
//! library under `RUMU_HOME/cards` (`@rumu:/cards/<id>`). The card library
//! itself — content model, storage, discovery — lives in the `rumu-cards`
//! crate; this plugin surfaces cards to the model through four built-in
//! hooks:
//!
//! - `card-catalog` on `llm.pre`: the layered card index;
//! - `recall` on `prompt`: imports the cards whose text best matches the
//!   prompt (BM25F over `keywords:`, title, description and body), within a
//!   body budget and deduped per `(id, rev)`;
//! - `related-cards` on `tool.post`: a successful call touching a workspace
//!   or file imports the cards whose `scope:` covers it, most specific first.
//!   Imports are revision-aware: the path's latest use of a card decides
//!   whether its body is (re)injected;
//! - `card-updates` on `llm.pre`: cards whose current revision differs from
//!   the path's last-used revision get a one-line notice (no body), deduped
//!   per new revision by querying the path index.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::Result;
use async_trait::async_trait;
use rumu_cards::catalog::{self, CatalogInput, CatalogNote, CatalogScope, SessionTouch};
use rumu_cards::store::FsCardStore;
use rumu_cards::{Card, library, scope, usage};
use rumu_core::{
    Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, Plugin, RUMU_WORKSPACE,
    SessionId, Timestamp, Utf8TypedPath, Utf8TypedPathBuf, now,
};
use serde_json::{Value, json};

#[cfg(test)]
mod tests;

/// Built-in hook id for the layered card catalog (fires on `llm.pre`).
const CARD_CATALOG_HOOK: &str = "card-catalog";
/// Built-in hook id for update notices (fires on `llm.pre`, runs last).
const CARD_UPDATES_HOOK: &str = "card-updates";
/// Built-in hook id for scope cards (fires on successful tool calls).
const RELATED_CARDS_HOOK: &str = "related-cards";
/// Built-in hook id for prompt recall (fires on `prompt`).
const RECALL_HOOK: &str = "recall";
/// At most this many cards recall considers per prompt.
const RECALL_LIMIT: usize = 5;
/// Rendered-text budget for the bodies recall inlines in one prompt.
const RECALL_BUDGET: usize = 10_000;

const INSTRUCTIONS: &str = "Cards are your memory across conversations: markdown notes at \
`@rumu:/cards/<id>`, where the id is a slash path such as `dev/rumu/kernel`.\n\n\
Finding cards. A catalog note lists cards by title and description; its folded directories unfold \
in later notes once you touch a card under them or switch to a related workspace. Some cards \
arrive on their own, in full, as notes: those whose text matches the user's message, and those \
whose `scope:` covers a workspace or file you use — don't re-read them. For any other card that \
looks relevant, `read` it; `search` finds cards the catalog doesn't show. When a note says a card \
changed, re-read it if it still matters.\n\n\
Saving cards. Save what will matter in later conversations: the user's preferences and \
corrections, project facts and conventions, decisions with their reasons, procedures that worked. \
Don't save the state of the current task or anything easily re-derived from the files. `search` \
before creating a card, and update an existing one rather than adding a near-duplicate. Keep one \
topic per card, grouped with related cards under a shared directory.\n\n\
Every card's frontmatter has a `title`, a one-sentence `description` (shown in the catalog) and \
`keywords:` — the terms that should bring it to mind, as a YAML list (`keywords: [deploy, \
production]`); keywords, title, description and body are what message matching scores. Give \
project-specific cards a `scope:` — a workspace name (`proj`) or a workspace-relative glob \
(`proj:src/net/**`), one or a list — so they arrive when that workspace or those files are used. \
Link other cards as `@rumu:/cards/<id>`.";

/// The memory plugin. Card content is read from disk on demand.
pub struct MemoryPlugin {
    /// The home card library (`RUMU_HOME/cards`).
    store: Arc<FsCardStore>,
    /// Per-session [`SessionTouch`], keyed by the session's `updated_at` so a
    /// cached history is reused until that session changes.
    touch_cache: Mutex<HashMap<SessionId, (Timestamp, SessionTouch)>>,
}

impl MemoryPlugin {
    /// Open the card library under `RUMU_HOME` (a missing `cards/` is an
    /// empty library).
    pub fn new(home: &Path) -> Self {
        Self {
            store: Arc::new(FsCardStore::home(home)),
            touch_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Load a card from the home library on demand (content is never cached).
    fn load_card(&self, id: &str) -> Result<Option<Card>> {
        Ok(library::load_card(&self.store, id)?)
    }
}

/// The built-in hooks: the two `llm.pre` passes (catalog index, then update
/// notices) and the scope-card import on successful tool calls.
fn builtin_hooks() -> Vec<HookSpec> {
    vec![
        HookSpec {
            id: CARD_CATALOG_HOOK.into(),
            matcher: json!({ "event": "llm.pre" }),
            priority: i32::MAX - 1,
        },
        HookSpec {
            id: CARD_UPDATES_HOOK.into(),
            matcher: json!({ "event": "llm.pre" }),
            priority: i32::MAX,
        },
        HookSpec {
            id: RELATED_CARDS_HOOK.into(),
            matcher: json!({
                "event": "tool.post",
                "tool": ["read", "write", "edit", "list", "bash", "workspace_use"],
            }),
            priority: 0,
        },
        HookSpec {
            id: RECALL_HOOK.into(),
            matcher: json!({ "event": "prompt" }),
            priority: 0,
        },
    ]
}

#[async_trait]
impl Plugin for MemoryPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "memory".into(),
            instructions: Some(INSTRUCTIONS.into()),
            tools: vec![],
            hooks: builtin_hooks(),
        }
    }

    async fn on_hook(
        &self,
        hook_id: &str,
        event: &Event,
        cx: &Cx,
    ) -> rumu_core::Result<HookOutput> {
        let out = match hook_id {
            CARD_CATALOG_HOOK => self.catalog_notes(cx).await,
            CARD_UPDATES_HOOK => self.update_notices(cx).await,
            RELATED_CARDS_HOOK => self.related_cards(event, cx).await,
            RECALL_HOOK => self.recall(event, cx).await,
            _ => Ok(HookOutput::default()),
        };
        out.map_err(rumu_core::Error::from)
    }
}

impl MemoryPlugin {
    /// The `card-catalog` hook: the root index once per branch plus
    /// progressive directory / workspace unfolding.
    async fn catalog_notes(&self, cx: &Cx) -> Result<HookOutput> {
        let cards = library::load_all(&self.store)?;
        if cards.is_empty() {
            return Ok(HookOutput::default());
        }
        let path_nodes = cx.path()?;
        let state = cx.state()?;

        let mut mtimes: BTreeMap<String, rumu_core::Timestamp> = BTreeMap::new();
        for id in cards.keys() {
            if let Ok(Some(m)) = self.store.mtime(id) {
                mtimes.insert(id.clone(), m);
            }
        }
        let now_ms = now();

        // Use history from other sessions; hosts without session storage
        // (the plugins' test fakes) degrade to "no history". A session's
        // touch is cached until its `updated_at` changes.
        let mut sessions: Vec<SessionTouch> = Vec::new();
        if let Ok(all) = cx.history().sessions() {
            let mut cache = self
                .touch_cache
                .lock()
                .expect("session touch cache poisoned");
            let live: HashSet<SessionId> = all.iter().map(|s| s.id.clone()).collect();
            cache.retain(|id, _| live.contains(id));
            for s in all {
                if s.id == cx.session {
                    continue;
                }
                if let Some((at, touch)) = cache.get(&s.id)
                    && *at == s.updated_at
                {
                    sessions.push(touch.clone());
                    continue;
                }
                if let Ok((_, nodes, _)) = cx.history().session_tree(&s.id) {
                    let touch = catalog::extract_session_touch(&nodes);
                    cache.insert(s.id.clone(), (s.updated_at, touch.clone()));
                    sessions.push(touch);
                }
            }
        }

        let scores: BTreeMap<String, f64> = cards
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    catalog::score_card(id, mtimes.get(id).copied(), &sessions, now_ms),
                )
            })
            .collect();

        let unfolded: Vec<CatalogScope> = cx
            .index()?
            .all::<CatalogNote>()
            .into_iter()
            .map(|e| e.value.scope)
            .collect();
        let notes = catalog::plan_unfold_notes(&CatalogInput {
            cards: &cards,
            scores: &scores,
            path_nodes: &path_nodes,
            unfolded: &unfolded,
            default_workspace: state.default_workspace.as_deref(),
            sessions: &sessions,
            now_ms,
        });
        Ok(HookOutput {
            effects: notes
                .into_iter()
                .map(|n| Content::note_with(n.catalog.markdown(), &n.catalog))
                .collect(),
            decision: Decision::Pass,
        })
    }

    /// The `card-updates` hook: one-line notices for used cards whose current
    /// revision differs from the last-used one. A notice already on the path
    /// for that `(id, rev)` is not repeated.
    async fn update_notices(&self, cx: &Cx) -> Result<HookOutput> {
        let nodes = cx.path()?;
        let uses = usage::uses_on_path(&nodes);
        let seen = cx.index()?.all::<usage::CardUpdate>();
        let mut out = HookOutput::default();
        for (id, u) in &uses {
            let (rev, text) = match self.load_card(id)? {
                Some(c) if Some(c.revision.as_str()) == u.rev.as_deref() => continue,
                Some(c) => (
                    c.revision.clone(),
                    format!(
                        "Card `{RUMU_WORKSPACE}:/cards/{id}` (\"{}\") changed since this branch last read \
                         it (rev {} → {}). Re-read it if it is still relevant.",
                        c.title.as_deref().unwrap_or(id),
                        u.rev.as_deref().unwrap_or("?"),
                        c.revision,
                    ),
                ),
                None => (
                    "deleted".to_owned(),
                    format!(
                        "Card `{RUMU_WORKSPACE}:/cards/{id}` was deleted after this branch last read it \
                         (last seen rev {}); its snapshot in this conversation is unchanged.",
                        u.rev.as_deref().unwrap_or("?"),
                    ),
                ),
            };
            if seen.iter().any(|e| e.value.id == *id && e.value.rev == rev) {
                continue;
            }
            out.effects.push(Content::note_with(
                text,
                &usage::CardUpdate {
                    id: id.clone(),
                    rev,
                },
            ));
        }
        Ok(out)
    }

    /// The `related-cards` hook: a successful call targeting a workspace
    /// imports the cards whose `scope:` covers it (or the touched files),
    /// most specific first. A card already seen on this path at its current
    /// revision is skipped; an older revision is superseded.
    async fn related_cards(&self, event: &Event, cx: &Cx) -> Result<HookOutput> {
        if event.payload.get("is_error").and_then(Value::as_bool) == Some(true) {
            return Ok(HookOutput::default());
        }
        // File tools and bash name the workspace in `fields.workspace`;
        // `workspace_use` carries it in its args.
        let name = if event.fields.tool.as_deref() == Some("workspace_use") {
            event.payload["args"]["name"].as_str().map(str::to_owned)
        } else {
            event.fields.workspace.clone()
        };
        let Some(name) = name.filter(|n| n != RUMU_WORKSPACE) else {
            return Ok(HookOutput::default());
        };
        let state = cx.state()?;
        let Some(ws) = state.workspaces.iter().find(|w| w.name == name) else {
            return Ok(HookOutput::default());
        };
        let root = ws.root.to_path();
        let rels: Vec<Utf8TypedPathBuf> = event
            .fields
            .paths
            .iter()
            .filter_map(|p| {
                Utf8TypedPath::new(p, ws.path_type())
                    .strip_prefix(root)
                    .ok()
                    .map(|rel| rel.with_unix_encoding())
            })
            .collect();
        let paths: Vec<&str> = rels.iter().map(|p| p.as_str()).collect();

        let cards = library::load_all(&self.store)?;
        let related = scope::related(&cards, &name, &paths);
        if related.is_empty() {
            return Ok(HookOutput::default());
        }
        let uses = usage::uses_on_path(&cx.path()?);
        let mut out = HookOutput::default();
        for card in related {
            let last = uses.get(&card.id).and_then(|u| u.rev.as_deref());
            let Some(body) = import_body(card, last) else {
                continue;
            };
            out.effects.push(Content::note_with(
                body,
                &usage::CardImport {
                    id: card.id.clone(),
                    rev: card.revision.clone(),
                },
            ));
        }
        Ok(out)
    }

    /// The `recall` hook: the cards whose text best matches the prompt are
    /// imported as notes, best first, until [`RECALL_BUDGET`] is spent.
    /// Revision-aware like scope imports: a card already on the path at the
    /// current revision is skipped, an older one is superseded.
    async fn recall(&self, event: &Event, cx: &Cx) -> Result<HookOutput> {
        let Some(text) = event.fields.text.as_deref() else {
            return Ok(HookOutput::default());
        };
        let cards = library::load_all(&self.store)?;
        let hits = rumu_cards::recall::rank(&cards, text, RECALL_LIMIT);
        if hits.is_empty() {
            return Ok(HookOutput::default());
        }
        let uses = usage::uses_on_path(&cx.path()?);
        let mut out = HookOutput::default();
        let mut budget = RECALL_BUDGET;
        for hit in hits {
            let card = hit.card;
            let last = uses.get(&card.id).and_then(|u| u.rev.as_deref());
            let Some(body) = import_body(card, last) else {
                continue;
            };
            if body.len() > budget {
                continue;
            }
            budget -= body.len();
            out.effects.push(Content::note_with(
                body,
                &usage::CardImport {
                    id: card.id.clone(),
                    rev: card.revision.clone(),
                },
            ));
        }
        Ok(out)
    }
}

/// The revision-aware body to import: `None` when the branch already saw this
/// revision (an import or a read — both are usage records); an "Updated card"
/// prefix when it saw an older one.
fn import_body(card: &Card, last: Option<&str>) -> Option<String> {
    match last {
        Some(rev) if rev == card.revision => None,
        Some(old) => Some(format!(
            "(Updated card — supersedes rev {old} read earlier on this branch.)\n{}",
            card.render()
        )),
        None => Some(card.render()),
    }
}
