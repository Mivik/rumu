//! Deck configuration, shared by the web (`assistant.upsert`,
//! `project.update`) and the model's `@rumu:/decks` writes: both change a
//! deck through these, so the two cannot drift apart.

use std::path::Path;

use rumu_cards::{Deck, DeckCat, DeckIcon, DeckMeta, DeckMount, Decks};
use rumu_core::{McpUse, ModelUse, Workspaces};

use crate::Result;
use crate::workspaces::set_project_workspaces;

/// An assistant's `deck.yaml` fields: everything but its id, its privacy
/// and its seal, which saving keeps.
#[derive(Debug, Clone, Default)]
pub struct AssistantConfig {
    pub name: String,
    pub mounts: Vec<DeckMount>,
    pub allow_ask: Vec<String>,
    pub mcp: McpUse,
    pub skills: Vec<String>,
    pub cat: Option<DeckCat>,
    pub model: Option<ModelUse>,
}

impl AssistantConfig {
    /// The fields `deck` has now.
    pub fn of(deck: &Deck) -> Self {
        let m = &deck.meta;
        Self {
            name: m.name.clone(),
            mounts: m.mounts.clone(),
            allow_ask: m.allow_ask.clone(),
            mcp: m.mcp.clone(),
            skills: m.skills.clone(),
            cat: m.cat.clone(),
            model: m.model.clone(),
        }
    }
}

/// Create (`existing` none) or update an assistant deck's `deck.yaml`. Its
/// pinned cards are left alone.
pub fn save_assistant(
    decks: &Decks,
    existing: Option<&Deck>,
    config: AssistantConfig,
) -> Result<Deck> {
    Ok(decks.upsert(DeckMeta {
        id: existing.map(|d| d.id().to_owned()).unwrap_or_default(),
        name: config.name,
        assistant: true,
        project: false,
        private: existing.is_some_and(Deck::private),
        mounts: config.mounts,
        allow_ask: config.allow_ask,
        model: config.model,
        mcp: config.mcp,
        skills: config.skills,
        icon: existing.and_then(|d| d.meta.icon.clone()),
        cat: config.cat,
    })?)
}

/// A change to a project deck: each field given is applied.
#[derive(Debug, Clone, Default)]
pub struct ProjectChange {
    pub name: Option<String>,
    /// The workspaces it mounts, in order (the first is its default).
    pub workspaces: Option<Vec<String>>,
    /// Its seal; an empty one clears it.
    pub icon: Option<DeckIcon>,
    pub private: Option<bool>,
}

/// Rename a project, set its seal and privacy, and/or replace the
/// workspaces it mounts. `deck.yaml` is written only when it changes.
pub fn update_project(
    home: &Path,
    decks: &Decks,
    registry: &Workspaces,
    deck: &Deck,
    change: ProjectChange,
) -> Result<Deck> {
    let mut meta = deck.meta.clone();
    if let Some(name) = change.name {
        meta.name = name;
    }
    if let Some(icon) = change.icon {
        meta.icon = icon.normalized();
    }
    if let Some(private) = change.private {
        meta.private = private;
    }
    let deck = if meta == deck.meta {
        deck.clone()
    } else {
        decks.upsert(meta)?
    };
    if let Some(names) = change.workspaces {
        set_project_workspaces(home, registry, deck.id(), names)?;
    }
    Ok(deck)
}
