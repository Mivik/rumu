//! The memory plugin.
//!
//! Memory is the capability of keeping, finding and using **cards**: markdown
//! files in the **decks** mounted on the branch (`@rumu:/cards/<mount>/<path>`).
//! The decks, their identity and history live in the `rumu-cards` crate; this
//! plugin surfaces their cards to the model through four built-in hooks:
//!
//! - `assistant-intro` on `llm.pre`: once per path, which assistant deck this
//!   is, the mount table, the placement rules and the mounted decks' pinned
//!   cards;
//! - `card-catalog` on `llm.pre`: one catalog per mounted deck (once per
//!   path);
//! - `recall` on `prompt`: imports the cards whose text best matches the
//!   prompt (BM25F over `keywords:`, path, description and body);
//! - `card-updates` on `llm.pre`: cards (keyed by deck id and path) whose current
//!   revision differs from the path's last-used revision get a one-line
//!   notice.

use std::collections::BTreeSet;

use crate::Result;
use async_trait::async_trait;
use rumu_cards::catalog::{self, CatalogNote};
use rumu_cards::deck::Action;
use rumu_cards::{Card, Cards, Decks, card, usage};
use rumu_core::{
    Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, PROJECT_MOUNT, Plugin,
    RUMU_WORKSPACE, Structured,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::approve::allowed;

#[cfg(test)]
mod tests;

/// Built-in hook id for the assistant's introduction (fires on `llm.pre`, first).
const ASSISTANT_INTRO_HOOK: &str = "assistant-intro";
/// Built-in hook id for the per-deck card catalogs (fires on `llm.pre`).
const CARD_CATALOG_HOOK: &str = "card-catalog";
/// Built-in hook id for update notices (fires on `llm.pre`, runs last).
const CARD_UPDATES_HOOK: &str = "card-updates";
/// Built-in hook id for prompt recall (fires on `prompt`).
const RECALL_HOOK: &str = "recall";
/// At most this many cards recall considers per prompt.
const RECALL_LIMIT: usize = 5;
/// Rendered-text budget for the bodies recall inlines in one prompt.
const RECALL_BUDGET: usize = 10_000;

const INSTRUCTIONS: &str = "Cards are your memory across conversations: markdown notes at \
`@rumu:/cards/<mount>/<path>`. The decks mounted on this branch appear under `/cards`; your own \
deck is `/cards/self`, and `/cards/<other>` are other mounted decks. Writing to another deck asks \
the user unless they already allowed it. Where a card belongs is in the placement rules of your \
introduction.\n\n\
Finding cards. A catalog note lists a deck's most-used cards by path and description; its folded \
directories group the rest — `list` or `search` reach them. Some cards \
arrive on their own, in full, as notes: those whose text matches the user's message — don't \
re-read them. For any other card that looks relevant, `read` it; `search` finds cards the catalog \
doesn't show. When a note says a card changed, re-read it if it still matters.\n\n\
Saving cards. Save what will matter in later conversations: the user's preferences and \
corrections, project facts and conventions, decisions with their reasons, procedures that worked. \
Don't save the state of the current task or anything easily re-derived from the files. `search` \
before creating a card, and update an existing one rather than adding a near-duplicate. Keep one \
topic per card, grouped with related cards under a shared directory. Name paths with short \
English kebab-case segments, such as `user/profile`.\n\n\
Every card's frontmatter has a one-sentence `description` (shown in the catalog) and \
`keywords:` — the terms that should bring it to mind, as a YAML list (`keywords: [deploy, \
production]`); keywords, description and body are what message matching scores. Write a card's \
description and body in the language of the conversation. Give `keywords` both the words the user \
actually uses for the topic, in their language, and their English equivalents. A link card is only `link: @rumu:/cards/<path>` (anything else is dropped); reads follow it, writes are \
refused.";

/// Payload of the `assistant.intro` note (v1): the once-per-path assistant introduction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AssistantIntro {
    name: String,
}

impl Structured for AssistantIntro {
    const TAG: &'static str = "assistant.intro";
    const V: u32 = 1;
}

/// The memory plugin. Card content is read from disk on demand.
pub struct MemoryPlugin {
    decks: Decks,
}

impl MemoryPlugin {
    pub fn new(decks: Decks) -> Self {
        Self { decks }
    }

    /// The mounted card space of the branch.
    fn cards(&self, cx: &Cx) -> Result<Cards> {
        let state = cx.state()?;
        Ok(Cards::new(&self.decks, &state.mounts))
    }
}

/// The built-in hooks: the `llm.pre` passes (introduction, catalog index,
/// then update notices) and prompt recall.
fn builtin_hooks() -> Vec<HookSpec> {
    vec![
        HookSpec {
            id: ASSISTANT_INTRO_HOOK.into(),
            matcher: json!({ "event": "llm.pre" }),
            priority: i32::MAX - 2,
        },
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
            ASSISTANT_INTRO_HOOK => self.assistant_intro(cx).await,
            CARD_CATALOG_HOOK => self.catalog_notes(cx).await,
            CARD_UPDATES_HOOK => self.update_notices(cx).await,
            RECALL_HOOK => self.recall(event, cx).await,
            _ => Ok(HookOutput::default()),
        };
        out.map_err(rumu_core::Error::from)
    }
}

impl MemoryPlugin {
    /// The `assistant-intro` hook: once per path, say which assistant deck
    /// this is, its mount table, the placement rules and the mounted decks'
    /// pinned cards. Written as a note, so the prompt cache is unaffected.
    async fn assistant_intro(&self, cx: &Cx) -> Result<HookOutput> {
        let state = cx.state()?;
        let Some(deck) = self.decks.get(&state.assistant) else {
            return Ok(HookOutput::default());
        };
        if !cx.index()?.all::<AssistantIntro>().is_empty() {
            return Ok(HookOutput::default());
        }
        let mut text = format!(
            "You are the assistant `{}` (deck id {}). Cards live in the decks mounted under `{RUMU_WORKSPACE}:/cards`.",
            deck.meta.name, deck.meta.id
        );
        match state.project.as_deref() {
            Some(project) => {
                let name = self.decks.get(project).map_or_else(
                    || project.to_owned(),
                    |d| format!("`{}` (deck id {})", d.meta.name, d.id()),
                );
                text.push_str(&format!(
                    "\nProject: {name}, mounted at `{RUMU_WORKSPACE}:{PROJECT_MOUNT}`."
                ));
                let mounted: Vec<_> = state
                    .mounted
                    .iter()
                    .filter_map(|n| state.workspaces.iter().find(|w| &w.name == n))
                    .collect();
                if mounted.is_empty() {
                    text.push_str(" It mounts no workspace; `@scratch` is the default.");
                } else {
                    text.push_str(" Its workspaces (default first): ");
                    text.push_str(
                        &mounted
                            .iter()
                            .map(|w| {
                                let rw = if w.read_only { "ro" } else { "rw" };
                                format!("`{}` ({} {}, {rw})", w.name, w.label(), w.root)
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                    text.push('.');
                }
            }
            None => text.push_str("\nThis session is in no project."),
        }
        let cards = Cards::new(&self.decks, &state.mounts);
        if cards.mounts().is_empty() {
            text.push_str(" No deck is mounted.");
        } else {
            text.push_str("\nMounts:");
            let session = cx.session.to_string();
            for mounted in cards.mounts() {
                let asks = state.asked_by.is_none()
                    && !allowed(
                        &self.decks,
                        &state,
                        &session,
                        Action::Write,
                        mounted.deck.id(),
                    );
                text.push_str(&format!(
                    "\n- `{RUMU_WORKSPACE}:/cards/{}` — deck `{}`{}",
                    mounted.point(),
                    mounted.deck.meta.name,
                    if asks { " (writes ask the user)" } else { "" }
                ));
            }
        }
        text.push_str(
            "\n\nPlacement rules:\n\
1. Put a card in the mounted deck whose charter (its pinned `about` card) fits what the card is \
about, whichever assistant learned it: a fact about the user goes to the user's deck, a fact \
about the session's project goes to `/cards/project`.\n\
2. Your own deck (`/cards/self`) is your own memory: your persona and instructions, what you \
learned from your own work (approaches that worked, mistakes to avoid), and work under way outside \
any project. A card no mounted deck fits also stays there until one does; then move it (`write` \
with `from`).\n\
3. Process stays in sessions (`@rumu:/sessions` is memory too); only conclusions become cards.\n\
4. Link related cards instead of copying them. In a card's body, link a card of the same deck by \
its path relative to the card (`[deploy](../ops/deploy)`), never by its mount path. For a card in \
another deck, write a link card in this deck (`link: @rumu:/cards/<path>`) and link to that.\n\
5. A deck's structure lives in its charter: when you add a directory or change what one holds, \
update the charter (what each directory holds, what to read first). Keep no separate index card: \
the catalog lists the cards.",
        );
        if state.asked_by.is_none() {
            let others: Vec<String> = self
                .decks
                .assistants()
                .into_iter()
                .filter(|d| d.id() != deck.id())
                .map(|d| match d.summary() {
                    Some(s) => format!("`{}` (id {}: {s})", d.meta.name, d.id()),
                    None => format!("`{}` (id {})", d.meta.name, d.id()),
                })
                .collect();
            if !others.is_empty() {
                text.push_str(&format!(
                    "\n\nOther assistants (`ask` takes the id): {}.",
                    others.join(", ")
                ));
            }
        }
        let mut pinned = Vec::new();
        for mounted in cards.mounts() {
            for card in mounted.deck.pinned()? {
                let public = mounted.card_path(&card.path);
                pinned.push(format!("## {public}\n{}", card.body.trim_end()));
            }
        }
        if !pinned.is_empty() {
            text.push_str("\n\nPinned cards:\n\n");
            text.push_str(&pinned.join("\n\n"));
        }
        Ok(HookOutput {
            effects: vec![Content::note_with(
                text,
                &AssistantIntro {
                    name: deck.meta.name.clone(),
                },
            )],
            decision: Decision::Pass,
        })
    }

    /// The `card-catalog` hook: one note per mounted deck at the first model
    /// call of a path that has no catalog yet. A catalog is frozen, so once a
    /// note is on the path this does nothing and reads no cards.
    async fn catalog_notes(&self, cx: &Cx) -> Result<HookOutput> {
        if !cx.index()?.all::<CatalogNote>().is_empty() {
            return Ok(HookOutput::default());
        }
        let cards = self.cards(cx)?;
        let state = cx.state()?;
        let now_ms = rumu_core::now();
        let mut effects = Vec::new();
        for mounted in cards.mounts() {
            let note = catalog::deck_catalog(
                &self.decks,
                &mounted.deck,
                &mounted.mount.at,
                &state.assistant,
                now_ms,
            )?;
            if !note.entries.is_empty() || !note.folded.is_empty() {
                effects.push(Content::note_with(note.markdown(), &note));
            }
        }
        Ok(HookOutput {
            effects,
            decision: Decision::Pass,
        })
    }

    /// The `card-updates` hook: one-line notices for used cards whose current
    /// revision differs from the last-used one. A notice already on the path
    /// for that `(deck, path, rev)` is not repeated.
    async fn update_notices(&self, cx: &Cx) -> Result<HookOutput> {
        let cards = self.cards(cx)?;
        let nodes = cx.path()?;
        let uses = usage::uses_on_path(&nodes);
        let seen = cx.index()?.all::<usage::CardUpdate>();
        let mounted = cards.deck_ids();
        // Live cards outside the mounted decks that a mounted link still
        // shares; read only when a changed card's deck is not mounted.
        let mut linked: Option<BTreeSet<(String, String)>> = None;
        let mut out = HookOutput::default();
        for ((deck_id, path), u) in &uses {
            let deck = self.decks.get(deck_id);
            let card = deck
                .as_ref()
                .and_then(|deck| deck.read(path).ok().flatten())
                .map(|text| card::parse_card(path, &text));
            let shown = cards.public_path(deck_id, path).unwrap_or_else(|| {
                let name = deck
                    .as_ref()
                    .map(|d| d.meta.name.clone())
                    .unwrap_or_else(|| deck_id.clone());
                format!("{name}:{path}")
            });
            let (rev, text) = match card {
                Some(c) if Some(c.revision.as_str()) == u.rev.as_deref() => continue,
                Some(c) => {
                    // An unmounted card is no longer updated.
                    if !mounted.contains(deck_id) {
                        if linked.is_none() {
                            linked = Some(cards.identities()?.into_values().collect());
                        }
                        if !linked
                            .as_ref()
                            .is_some_and(|l| l.contains(&(deck_id.clone(), path.clone())))
                        {
                            continue;
                        }
                    }
                    (
                        c.revision.clone(),
                        format!(
                            "Card `{RUMU_WORKSPACE}:/cards/{shown}` changed since this branch last \
                             read it (rev {} → {}). Re-read it if it is still relevant.",
                            u.rev.as_deref().unwrap_or("?"),
                            c.revision,
                        ),
                    )
                }
                None => {
                    // The card is gone; its path is where it was.
                    if !mounted.contains(deck_id) {
                        continue;
                    }
                    (
                        "deleted".to_owned(),
                        format!(
                            "Card `{RUMU_WORKSPACE}:/cards/{shown}` was deleted after this branch last read it (last seen rev {}); its snapshot in this conversation is unchanged.",
                            u.rev.as_deref().unwrap_or("?"),
                        ),
                    )
                }
            };
            if seen
                .iter()
                .any(|e| e.value.deck == *deck_id && e.value.path == *path && e.value.rev == rev)
            {
                continue;
            }
            out.effects.push(Content::note_with(
                text,
                &usage::CardUpdate {
                    deck: deck_id.clone(),
                    path: path.clone(),
                    rev,
                },
            ));
        }
        Ok(out)
    }

    /// The `recall` hook: the cards whose text best matches the prompt are
    /// imported as notes, best first, until [`RECALL_BUDGET`] is spent.
    async fn recall(&self, event: &Event, cx: &Cx) -> Result<HookOutput> {
        let Some(text) = event.fields.text.as_deref() else {
            return Ok(HookOutput::default());
        };
        let cards = self.cards(cx)?;
        let all = cards.rankable()?;
        let hits = rumu_cards::recall::rank(&all, text, RECALL_LIMIT);
        if hits.is_empty() {
            return Ok(HookOutput::default());
        }
        let uses = usage::uses_on_path(&cx.path()?);
        let identities = cards.identities()?;
        let mut out = HookOutput::default();
        let mut budget = RECALL_BUDGET;
        for hit in hits {
            let card = hit.card;
            let Some((deck, path)) = identities.get(&card.path) else {
                continue;
            };
            let last = uses
                .get(&(deck.clone(), path.clone()))
                .and_then(|u| u.rev.as_deref());
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
                    deck: deck.clone(),
                    path: path.clone(),
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
