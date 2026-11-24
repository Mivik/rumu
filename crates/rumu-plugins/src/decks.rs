//! Deck configuration, shared by the web (`assistant.upsert`,
//! `project.update`) and the model's `@rumu:/decks` writes: both change a
//! deck through these, so the two cannot drift apart.

use std::path::Path;

use rumu_cards::{Deck, DeckCat, DeckIcon, DeckMeta, DeckMount, Decks};
use rumu_core::{McpUse, ModelUse, PathState, Workspaces};

use crate::Result;
use crate::workspaces::set_project_workspaces;

/// The decks whose MCP servers and skills a path uses, as they are now: its
/// assistant's, then its project's. A change to either reaches its running
/// sessions at their next model call.
pub(crate) fn binding(decks: &Decks, state: &PathState) -> Vec<DeckMeta> {
    [Some(state.assistant.as_str()), state.project()]
        .into_iter()
        .flatten()
        .filter_map(|id| decks.get(id))
        .map(|d| d.meta)
        .collect()
}

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

impl From<DeckMeta> for AssistantConfig {
    fn from(m: DeckMeta) -> Self {
        Self {
            name: m.name,
            mounts: m.mounts,
            allow_ask: m.allow_ask,
            mcp: m.mcp,
            skills: m.skills,
            cat: m.cat,
            model: m.model,
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
    /// The MCP servers its sessions use besides their assistant's.
    pub mcp: Option<McpUse>,
    /// The rules enabling skills for its sessions besides their assistant's.
    pub skills: Option<Vec<String>>,
}

/// Create a project deck (blank charter) with `change` applied; it needs a
/// name.
pub fn create_project(
    home: &Path,
    decks: &Decks,
    registry: &Workspaces,
    change: ProjectChange,
) -> Result<Deck> {
    let deck = decks.upsert(DeckMeta {
        id: String::new(),
        name: change.name.unwrap_or_default(),
        assistant: false,
        project: true,
        private: change.private.unwrap_or_default(),
        mounts: Vec::new(),
        allow_ask: Vec::new(),
        model: None,
        mcp: change.mcp.unwrap_or_default(),
        skills: change.skills.unwrap_or_default(),
        icon: change.icon,
        cat: None,
    })?;
    set_project_workspaces(
        home,
        registry,
        deck.id(),
        change.workspaces.unwrap_or_default(),
    )?;
    Ok(deck)
}

/// Rename a project, set its seal, privacy, MCP servers and skills, and/or
/// replace the workspaces it mounts. `deck.yaml` is written only when it
/// changes.
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
    if let Some(mcp) = change.mcp {
        meta.mcp = mcp;
    }
    if let Some(skills) = change.skills {
        meta.skills = skills;
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
