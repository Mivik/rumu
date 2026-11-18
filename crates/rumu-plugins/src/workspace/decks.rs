//! `@rumu:/decks`: the decks this session may see, by name, and each one's
//! configuration as YAML.
//!
//! - `list /decks`: each deck's name, kind, where it is mounted, whether
//!   mounting or writing it asks the user, and its charter's summary;
//! - `read /decks/<name>`: its configuration, by kind (an assistant's model,
//!   MCP servers, skills, mounts, `allow_ask` and mascot; a project's seal,
//!   privacy and workspaces; a plain deck's privacy). Decks are named, never
//!   given by id. Its persona and charter are its pinned cards, not here;
//! - `write` changes it after a read (a field left out keeps its value),
//!   `edit` replaces text in the current one, `write` to a new name creates
//!   a deck (`kind:` says which), and `write` with `from` renames one.
//!   Deleting a deck is the user's, in the web UI.
//!
//! The web edits the same fields through `assistant.upsert` and
//! `project.update`, and both go through [`crate::decks`].

use async_trait::async_trait;
use rumu_cards::deck::{Action, Scope, valid_deck_name};
use rumu_cards::{Cards, Deck, DeckCat, DeckIcon, DeckMeta, DeckMount};
use rumu_core::{
    Content, McpUse, ModelUse, PathState, RUMU_WORKSPACE, Structured, ToolError, ToolOutput,
    ToolResult,
};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;

use crate::approve::{allowed, approve, confirm, deck_name};
use crate::decks::{ProjectChange, save_assistant, update_project};
use crate::workspaces::create_project;

use super::WorkspacePlugin;
use super::tools::replace_occurrences;
use super::vfs::{At, Node, Out};

/// Payload of the read marker on a configuration file (v1): its path and
/// the content hash the last read returned. A write must be based on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConfigRead {
    pub path: String,
    pub hash: String,
}

impl Structured for ConfigRead {
    const TAG: &'static str = "config.read";
    const V: u32 = 1;
}

/// The read marker for `path` showing `text`: the base of the next write
/// (a write of one's own is as good as a read).
fn read_marker(path: &str, text: &str) -> Content {
    Content::marker(&ConfigRead {
        path: path.to_owned(),
        hash: rumu_cards::card::view_rev_of(text),
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Assistant,
    Project,
    #[default]
    Deck,
}

impl Kind {
    fn of(meta: &DeckMeta) -> Self {
        if meta.assistant {
            Self::Assistant
        } else if meta.project {
            Self::Project
        } else {
            Self::Deck
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Assistant => "assistant",
            Self::Project => "project",
            Self::Deck => "deck",
        }
    }

    /// The fields a deck of this kind has, besides `kind`.
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Assistant => &["model", "mcp", "skills", "mounts", "allow_ask", "cat"],
            Self::Project => &["icon", "private", "workspaces"],
            Self::Deck => &["private"],
        }
    }
}

/// A mount as the model sees it: the deck by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MountView {
    deck: String,
    at: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    write: bool,
}

/// What `read` shows: the fields of the deck's kind (lists and maps even
/// when empty, so they can be filled in).
#[derive(Debug, Default, Serialize)]
struct Shown {
    kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<ModelUse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<McpUse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mounts: Option<Vec<MountView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    allow_ask: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cat: Option<DeckCat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<DeckIcon>,
    #[serde(skip_serializing_if = "Option::is_none")]
    private: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspaces: Option<Vec<String>>,
}

/// What a write gives: each field left out keeps its value; `null` clears
/// `model`, `cat` and `icon`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    kind: Option<Kind>,
    #[serde(default, deserialize_with = "present")]
    model: Option<Option<ModelUse>>,
    mcp: Option<McpUse>,
    skills: Option<Vec<String>>,
    mounts: Option<Vec<MountView>>,
    allow_ask: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present")]
    cat: Option<Option<DeckCat>>,
    #[serde(default, deserialize_with = "present")]
    icon: Option<Option<DeckIcon>>,
    private: Option<bool>,
    workspaces: Option<Vec<String>>,
}

/// A field that is there, `null` or not (`None` when it is left out).
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

impl Written {
    fn parse(text: &str) -> ToolResult<Self> {
        let written: Option<Self> = rumu_cards::yaml::from_str(text)
            .map_err(|e| ToolError::model(format!("the deck does not parse: {e}")))?;
        Ok(written.unwrap_or_default())
    }

    /// The fields given, by name.
    fn given(&self) -> Vec<&'static str> {
        let all = [
            ("model", self.model.is_some()),
            ("mcp", self.mcp.is_some()),
            ("skills", self.skills.is_some()),
            ("mounts", self.mounts.is_some()),
            ("allow_ask", self.allow_ask.is_some()),
            ("cat", self.cat.is_some()),
            ("icon", self.icon.is_some()),
            ("private", self.private.is_some()),
            ("workspaces", self.workspaces.is_some()),
        ];
        all.into_iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| n)
            .collect()
    }

    /// Refuse the fields a deck of `kind` does not have.
    fn check(&self, kind: Kind) -> ToolResult<()> {
        let foreign: Vec<&str> = self
            .given()
            .into_iter()
            .filter(|f| !kind.fields().contains(f))
            .collect();
        if foreign.is_empty() {
            return Ok(());
        }
        Err(ToolError::model(format!(
            "{} {} not a field of a{} {}; its fields: kind, {}",
            foreign.join(", "),
            if foreign.len() == 1 { "is" } else { "are" },
            if kind == Kind::Assistant { "n" } else { "" },
            kind.name(),
            kind.fields().join(", ")
        )))
    }
}

/// One deck's configuration as the model reads and writes it: its
/// `deck.yaml` and, for a project, the workspaces it mounts.
#[derive(Debug, Clone, PartialEq)]
struct Config {
    meta: DeckMeta,
    workspaces: Vec<String>,
}

impl WorkspacePlugin {
    /// The deck named `name` (any case), if this session may see it.
    fn deck_named(&self, state: &PathState, name: &str) -> Option<Deck> {
        let name = name.trim().to_lowercase();
        self.decks()
            .list()
            .into_iter()
            .find(|d| d.meta.name.to_lowercase() == name)
            .filter(|d| {
                self.decks()
                    .visible(d.id(), &state.assistant, &state.mounts)
            })
    }

    fn config_of(&self, deck: &Deck) -> Config {
        Config {
            meta: deck.meta.clone(),
            workspaces: if deck.project() {
                self.workspaces.project(deck.id())
            } else {
                Vec::new()
            },
        }
    }

    /// The YAML `read` shows for `config`. Mounts and `allow_ask` name
    /// decks; ones this session may not see are left out (and kept on a
    /// write).
    fn shown(&self, state: &PathState, config: &Config) -> String {
        let meta = &config.meta;
        let visible = |id: &str| -> Option<String> {
            self.decks()
                .get(id)
                .filter(|_| self.decks().visible(id, &state.assistant, &state.mounts))
                .map(|d| d.meta.name)
        };
        let kind = Kind::of(meta);
        let mut shown = Shown {
            kind,
            ..Shown::default()
        };
        match kind {
            Kind::Assistant => {
                shown.model = meta.model.clone();
                shown.mcp = Some(meta.mcp.clone());
                shown.skills = Some(meta.skills.clone());
                shown.mounts = Some(
                    meta.mounts
                        .iter()
                        .filter_map(|m| {
                            Some(MountView {
                                deck: visible(&m.deck)?,
                                at: m.at.clone(),
                                write: m.write,
                            })
                        })
                        .collect(),
                );
                shown.allow_ask =
                    Some(meta.allow_ask.iter().filter_map(|id| visible(id)).collect());
                shown.cat = meta.cat.clone();
            }
            Kind::Project => {
                shown.icon = meta.icon.clone();
                shown.private = Some(meta.private);
                shown.workspaces = Some(config.workspaces.clone());
            }
            Kind::Deck => shown.private = Some(meta.private),
        }
        serde_saphyr::to_string(&shown).unwrap_or_default()
    }

    /// `config` with `written` applied: decks named in mounts and
    /// `allow_ask` are resolved, and the entries this session cannot see
    /// are kept.
    fn apply(&self, state: &PathState, config: &Config, written: Written) -> ToolResult<Config> {
        let mut next = config.clone();
        let meta = &mut next.meta;
        let hidden = |id: &str| !self.decks().visible(id, &state.assistant, &state.mounts);
        if let Some(model) = written.model {
            meta.model = model;
        }
        if let Some(mcp) = written.mcp {
            meta.mcp = mcp;
        }
        if let Some(skills) = written.skills {
            meta.skills = skills;
        }
        if let Some(cat) = written.cat {
            meta.cat = cat;
        }
        if let Some(icon) = written.icon {
            meta.icon = icon.and_then(DeckIcon::normalized);
        }
        if let Some(private) = written.private {
            meta.private = private;
        }
        if let Some(mounts) = written.mounts {
            let mut list: Vec<DeckMount> = Vec::new();
            for m in mounts {
                let deck = self.named(state, &m.deck)?;
                list.push(DeckMount {
                    deck: deck.meta.id,
                    at: m.at,
                    write: m.write,
                });
            }
            list.extend(meta.mounts.iter().filter(|m| hidden(&m.deck)).cloned());
            meta.mounts = list;
        }
        if let Some(names) = written.allow_ask {
            let mut ids: Vec<String> = Vec::new();
            for name in names {
                let deck = self.named(state, &name)?;
                if !deck.assistant() {
                    return Err(ToolError::model(format!(
                        "allow_ask: `{}` is not an assistant",
                        deck.meta.name
                    )));
                }
                ids.push(deck.meta.id);
            }
            ids.extend(meta.allow_ask.iter().filter(|id| hidden(id)).cloned());
            meta.allow_ask = ids;
        }
        if let Some(names) = written.workspaces {
            next.workspaces = names;
        }
        Ok(next)
    }

    fn named(&self, state: &PathState, name: &str) -> ToolResult<Deck> {
        self.deck_named(state, name).ok_or_else(|| no_deck(name))
    }

    /// Save `next` over `deck` (renaming it when `next` has another name).
    fn save(&self, deck: &Deck, next: Config) -> ToolResult<Deck> {
        let err = |e: crate::Error| ToolError::model(e.to_string());
        match Kind::of(&deck.meta) {
            Kind::Assistant => {
                save_assistant(self.decks(), Some(deck), next.meta.into()).map_err(err)
            }
            Kind::Project => {
                let workspaces = next.workspaces;
                let change = ProjectChange {
                    name: Some(next.meta.name),
                    workspaces: (workspaces != self.workspaces.project(deck.id()))
                        .then_some(workspaces),
                    icon: Some(next.meta.icon.unwrap_or_default()),
                    private: Some(next.meta.private),
                };
                update_project(&self.home, self.decks(), &self.workspaces, deck, change)
                    .map_err(err)
            }
            Kind::Deck if next.meta == deck.meta => Ok(deck.clone()),
            Kind::Deck => self
                .decks()
                .upsert(next.meta)
                .map_err(|e| ToolError::model(e.to_string())),
        }
    }

    /// Ask what a change from `before` to `after` needs asked: the grants
    /// (`mounts`, `allow_ask`) always, without remembering; the MCP servers
    /// and privacy of any deck, and anything of a deck that is neither the
    /// assistant's own nor the session's project, as `configure`. `Ok(false)`
    /// when the user declines.
    async fn approve_change(
        &self,
        state: &PathState,
        deck: &Deck,
        before: &Config,
        after: &Config,
        shown: (String, String),
        cx: &rumu_core::Cx,
    ) -> ToolResult<bool> {
        let (b, a) = (&before.meta, &after.meta);
        let grants = b.mounts != a.mounts || b.allow_ask != a.allow_ask;
        let own = deck.id() == state.assistant || state.project.as_deref() == Some(deck.id());
        let rest = b.name != a.name
            || b.model != a.model
            || b.skills != a.skills
            || b.cat != a.cat
            || b.icon != a.icon
            || before.workspaces != after.workspaces;
        let configure = b.mcp != a.mcp || b.private != a.private || (rest && !own);
        let request = json!({
            "kind": "deck_change",
            "from": deck_name(self.decks(), &state.assistant),
            "from_id": state.assistant,
            "deck": deck.meta.name,
            "deck_id": deck.id(),
            "before": shown.0,
            "after": shown.1,
        });
        let asked = |e: ToolError| ToolError::model(format!("deck: {e}"));
        if grants {
            return confirm(cx, request).await.map_err(asked);
        }
        if configure {
            return approve(self.decks(), cx, Action::Configure, deck.id(), request)
                .await
                .map_err(asked);
        }
        Ok(true)
    }

    /// Change the deck `deck` (at `path`) to `written`, renamed to `name`.
    async fn deck_update(&self, at: &At<'_>, deck: &Deck, name: &str, written: Written) -> Out {
        let kind = Kind::of(&deck.meta);
        if written.kind.is_some_and(|k| k != kind) {
            return Err(ToolError::model(format!(
                "`{}` is a{} {}; a deck's kind does not change",
                deck.meta.name,
                if kind == Kind::Assistant { "n" } else { "" },
                kind.name()
            )));
        }
        written.check(kind)?;
        let before = self.config_of(deck);
        let mut after = self.apply(at.state, &before, written)?;
        after.meta.name = name.trim().to_owned();
        if after == before {
            let text = self.shown(at.state, &before);
            return Ok(ToolOutput::ok(format!("`{}` is unchanged", at.abs))
                .with_effect(read_marker(&deck_path(name), &text)));
        }
        let shown = (self.shown(at.state, &before), self.shown(at.state, &after));
        if !self
            .approve_change(at.state, deck, &before, &after, shown, at.cx)
            .await?
        {
            return Err(ToolError::model(format!(
                "the user declined the change to deck `{}`",
                deck.meta.name
            )));
        }
        let saved = self.save(deck, after)?;
        let text = self.shown(at.state, &self.config_of(&saved));
        let path = deck_path(&saved.meta.name);
        Ok(
            ToolOutput::ok(format!("wrote `{RUMU_WORKSPACE}:{path}`:\n{text}"))
                .with_effect(read_marker(&path, &text)),
        )
    }

    /// Create the deck `name` from `written` (its `kind` required).
    async fn deck_create(&self, at: &At<'_>, name: &str, written: Written) -> Out {
        let name = name.trim();
        if !valid_deck_name(name) {
            return Err(ToolError::model(format!(
                "invalid deck name `{name}` (1-{} characters, no control characters)",
                rumu_cards::deck::DECK_NAME_MAX
            )));
        }
        if self.decks().name_taken(name, None) {
            return Err(ToolError::model(format!(
                "a deck named `{name}` already exists"
            )));
        }
        let Some(kind) = written.kind else {
            return Err(ToolError::model(format!(
                "no deck `{name}`: writing one creates it, with `kind: assistant`, `project` or \
                 `deck`"
            )));
        };
        written.check(kind)?;
        let blank = Config {
            meta: DeckMeta {
                id: String::new(),
                name: name.to_owned(),
                assistant: kind == Kind::Assistant,
                project: kind == Kind::Project,
                private: false,
                mounts: Vec::new(),
                allow_ask: Vec::new(),
                model: None,
                mcp: Default::default(),
                skills: Vec::new(),
                icon: None,
                cat: None,
            },
            workspaces: Vec::new(),
        };
        let config = self.apply(at.state, &blank, written)?;
        let content = self.shown(at.state, &config);
        let request = json!({
            "kind": "deck_create",
            "from": deck_name(self.decks(), &at.state.assistant),
            "from_id": at.state.assistant,
            "name": name,
            "deck_kind": kind.name(),
            "content": content,
        });
        // Grants in a new assistant are asked about every time, like any
        // change to them.
        let grants = !config.meta.mounts.is_empty() || !config.meta.allow_ask.is_empty();
        let asked = |e: ToolError| ToolError::model(format!("deck: {e}"));
        let allowed = if grants {
            confirm(at.cx, request).await.map_err(asked)?
        } else {
            approve(self.decks(), at.cx, Action::Create, "*", request)
                .await
                .map_err(asked)?
        };
        if !allowed {
            return Err(ToolError::model(format!(
                "the user declined to create deck `{name}`"
            )));
        }
        let err = |e: crate::Error| ToolError::model(format!("creating deck `{name}`: {e}"));
        let deck = match kind {
            Kind::Project => create_project(
                &self.home,
                self.decks(),
                &self.workspaces,
                name,
                config.workspaces.clone(),
                config.meta.icon.clone(),
                config.meta.private,
            )
            .map_err(err)?,
            Kind::Assistant => {
                save_assistant(self.decks(), None, config.meta.clone().into()).map_err(err)?
            }
            Kind::Deck => {
                let created = rumu_cards::library::deck_create(self.decks(), name)
                    .map_err(|e| err(e.into()))?;
                let deck = self
                    .decks()
                    .get(&created.id)
                    .ok_or_else(|| ToolError::model(format!("creating deck `{name}`: gone")))?;
                if config.meta.private {
                    self.save(
                        &deck,
                        Config {
                            meta: DeckMeta {
                                private: true,
                                ..deck.meta.clone()
                            },
                            workspaces: Vec::new(),
                        },
                    )?
                } else {
                    deck
                }
            }
        };
        // Its creator's session may mount it and write it without asking
        // again: the user just approved the deck itself.
        let session = at.cx.session.to_string();
        for action in [Action::Mount, Action::Write] {
            self.decks().remember(
                action,
                Scope::Session,
                &session,
                &at.state.assistant,
                deck.id(),
            );
        }
        let path = deck_path(&deck.meta.name);
        let text = self.shown(at.state, &self.config_of(&deck));
        Ok(ToolOutput::ok(format!(
            "created {} `{}` with a blank charter (`{RUMU_WORKSPACE}:{path}`):\n{text}",
            kind.name(),
            deck.meta.name
        ))
        .with_effect(read_marker(&path, &text)))
    }
}

/// `/decks/<name>` (as the read marker records it).
fn deck_path(name: &str) -> String {
    format!("/decks/{name}")
}

fn no_deck(name: &str) -> ToolError {
    ToolError::model(format!(
        "no deck `{name}`; `list {RUMU_WORKSPACE}:/decks` lists them"
    ))
}

/// `/decks`: one line per deck the session may see.
pub(super) struct DecksDir;

#[async_trait]
impl Node for DecksDir {
    async fn list(&self, at: &At<'_>) -> Out {
        let (p, state) = (at.p, at.state);
        let cards = Cards::new(p.decks(), &state.mounts);
        let session = at.cx.session.to_string();
        let mut decks = p.decks().list();
        decks.sort_by_key(|d| d.meta.name.to_lowercase());
        let lines: Vec<String> = decks
            .iter()
            .filter(|d| p.decks().visible(d.id(), &state.assistant, &state.mounts))
            .map(|deck| {
                let mounted = cards
                    .mounts()
                    .iter()
                    .find(|m| m.deck.id() == deck.id())
                    .map(|m| format!("at {}", m.mount.at))
                    .unwrap_or_else(|| "not mounted".into());
                let asks: Vec<&str> = [(Action::Mount, "mount"), (Action::Write, "write")]
                    .into_iter()
                    .filter(|(action, _)| !allowed(p.decks(), state, &session, *action, deck.id()))
                    .map(|(_, name)| name)
                    .collect();
                format!(
                    "{}  {}  {mounted}{}{}",
                    deck.meta.name,
                    Kind::of(&deck.meta).name(),
                    if asks.is_empty() {
                        String::new()
                    } else {
                        format!("  asks the user to {}", asks.join(", "))
                    },
                    deck.summary()
                        .map(|s| format!("  — {s}"))
                        .unwrap_or_default(),
                )
            })
            .collect();
        Ok(if lines.is_empty() {
            "no decks".into()
        } else {
            lines.join("\n").into()
        })
    }

    fn holds_files(&self) -> bool {
        true
    }
}

/// `/decks/<name>`: one deck's configuration.
pub(super) struct DeckFile;

#[async_trait]
impl Node for DeckFile {
    async fn list(&self, at: &At<'_>) -> Out {
        let deck = at.p.named(at.state, at.param("name"))?;
        Ok(at.p.shown(at.state, &at.p.config_of(&deck)).into())
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let deck = at.p.named(at.state, at.param("name"))?;
        let text = at.p.shown(at.state, &at.p.config_of(&deck));
        let marker = read_marker(&deck_path(&deck.meta.name), &text);
        let sliced = super::tools::slice_lines(text, offset, limit).map_err(ToolError::model)?;
        Ok(ToolOutput::ok(sliced).with_effect(marker))
    }

    /// Change, create, or (with `from`) rename a deck. A change must be
    /// based on a read of the deck as it is now.
    async fn write(&self, at: &At<'_>, content: &str, from: Option<&str>) -> Out {
        let name = at.param("name");
        let written = Written::parse(content)?;
        let source = match from {
            Some(from) => {
                let old = from
                    .trim_start_matches(&format!("{RUMU_WORKSPACE}:"))
                    .strip_prefix("/decks/")
                    .filter(|n| !n.is_empty() && !n.contains('/'))
                    .ok_or_else(|| {
                        ToolError::model(format!(
                            "`from` names the deck to rename: `{RUMU_WORKSPACE}:/decks/<name>`"
                        ))
                    })?;
                let deck = at.p.named(at.state, old)?;
                if at.p.decks().name_taken(name, Some(deck.id())) {
                    return Err(ToolError::model(format!(
                        "a deck named `{}` already exists",
                        name.trim()
                    )));
                }
                deck
            }
            None => match at.p.deck_named(at.state, name) {
                Some(deck) => deck,
                None => return at.p.deck_create(at, name, written).await,
            },
        };
        let current = at.p.shown(at.state, &at.p.config_of(&source));
        let path = deck_path(&source.meta.name);
        let seen = at
            .cx
            .index()?
            .all::<ConfigRead>()
            .into_iter()
            .rev()
            .find(|e| e.value.path == path);
        if seen.map(|e| e.value.hash) != Some(rumu_cards::card::view_rev_of(&current)) {
            return Err(ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{path}` changed since you read it, or you have not read it; \
                 read it again first. It is now:\n{current}"
            )));
        }
        let new_name = if from.is_some() {
            name
        } else {
            &source.meta.name
        };
        at.p.deck_update(at, &source, new_name, written).await
    }

    /// Replace text in the deck as it is now, then save it.
    async fn edit(&self, at: &At<'_>, old: &str, new: &str, all: bool) -> Out {
        let deck = at.p.named(at.state, at.param("name"))?;
        let current = at.p.shown(at.state, &at.p.config_of(&deck));
        let (text, _) = replace_occurrences(&current, old, new, all)
            .map_err(|e| ToolError::model(e.to_string()))?;
        let written = Written::parse(&text)?;
        at.p.deck_update(at, &deck, &deck.meta.name, written).await
    }

    async fn delete(&self, at: &At<'_>) -> Out {
        Err(ToolError::model(format!(
            "`{RUMU_WORKSPACE}:{}` cannot be deleted: the user deletes decks in the web UI",
            at.abs
        )))
    }
}
