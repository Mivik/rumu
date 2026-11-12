//! Decks: bare git repositories of cards, and the registry that knows them.
//!
//! A deck lives at `RUMU_HOME/decks/<id>/`; its identity and defaults are in
//! `deck.yaml` in the `HEAD` tree. Decks are addressed by the `id` in
//! `deck.yaml`, never by their name; the name is only a display label. Which
//! assistant is the default is per user, in `RUMU_HOME/default-assistant`.
//! rumu is the only writer: a card write or a UI edit is a commit, so there
//! is no worktree to scan or reconcile.
//!
//! [`Decks`] loads the deck repositories and refreshes its in-memory view
//! after rumu's own commits. A card's frontmatter `id:` is its identity: it
//! survives moves, and the usage index is keyed by it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use rumu_core::CardMount;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::card::{
    Card, card_content, frontmatter_is_broken, new_uuid, parse_card, valid_card_path,
};
use crate::cards::{PROJECT_MOUNT, SELF_MOUNT, mount_points_overlap, normalize_mount_point};
use crate::git::{self, GitAuthor, Repo};
use crate::{Error, Result};

/// The directory holding every deck under `RUMU_HOME`.
pub const DECKS_DIR: &str = "decks";
/// The deck identity/defaults file.
pub const DECK_FILE: &str = "deck.yaml";
/// The grants file ("always allow" answers).
pub const GRANTS_FILE: &str = "grants.json";
/// The charter card: its body says what belongs in the deck, its
/// `description` is the deck's one-line summary.
pub const ABOUT: &str = "about";
/// The file holding the default assistant's deck id.
pub const DEFAULT_FILE: &str = "default-assistant";
/// The author recorded for changes made by the user (UI edits).
pub const USER: &str = "user";

/// The longest deck display name, in characters.
pub const DECK_NAME_MAX: usize = 80;

/// A deck's display name: trimmed, 1–[`DECK_NAME_MAX`] characters, no control
/// characters. Names are labels only — decks are stored and addressed by id,
/// so anything readable (spaces, capitals, CJK) is fine.
pub fn valid_deck_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty() && name.chars().count() <= DECK_NAME_MAX && !name.chars().any(char::is_control)
}

/// A deck's default mount entry (`deck.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeckMount {
    /// The mounted deck's id.
    pub deck: String,
    /// The mount point (`/cards/user`).
    pub at: String,
    /// Writes to the deck need no approval.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub write: bool,
}

/// The tones a project's seal can take (the web's semantic colors).
pub const ICON_TONES: &[&str] = &["brand", "tool", "pine", "memory", "agent", "ink"];

/// A project's seal (`deck.yaml icon`). Either part left out falls back: the
/// glyph to the name's first letter, the tone to one picked from the id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeckIcon {
    /// A letter, a CJK character or an emoji.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph: Option<String>,
    /// One of [`ICON_TONES`].
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tone: Option<String>,
}

impl DeckIcon {
    /// The icon with blank parts dropped, `None` when nothing is left.
    pub fn normalized(self) -> Option<Self> {
        let glyph = self
            .glyph
            .map(|g| g.trim().to_owned())
            .filter(|g| !g.is_empty());
        let tone = self.tone.filter(|t| !t.is_empty());
        (glyph.is_some() || tone.is_some()).then_some(Self { glyph, tone })
    }

    fn check(&self) -> std::result::Result<(), String> {
        if let Some(g) = &self.glyph
            && (g.chars().count() > 4
                || g.len() > 16
                || g.chars().any(|c| c.is_whitespace() || c.is_control()))
        {
            return Err(format!("invalid icon glyph `{g}` (one letter or emoji)"));
        }
        if let Some(t) = &self.tone
            && !ICON_TONES.contains(&t.as_str())
        {
            return Err(format!(
                "invalid icon tone `{t}` (one of {})",
                ICON_TONES.join(", ")
            ));
        }
        Ok(())
    }
}

/// What one deck may do to another without asking the user, once a grant
/// covers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Mount the deck, which makes it readable.
    Mount,
    /// Write, move or delete the deck's cards.
    Write,
    /// Ask the assistant deck a question.
    Ask,
    /// Link one of the deck's cards from another deck.
    Link,
    /// Create a project deck (no target deck: grants name `*`).
    Create,
    /// Change a registered workspace (the target is its name).
    Workspace,
}

/// How long the user's approval lasts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// This one action.
    #[default]
    Once,
    /// The rest of the session. Kept in memory: after a restart rumu asks again.
    Session,
    /// From now on, saved in `grants.json`.
    Always,
}

/// `deck.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckMeta {
    /// UUIDv4, assigned by rumu.
    pub id: String,
    /// Display name only; decks are always referred to by id.
    pub name: String,
    /// Listed in the roster; sessions can run as it.
    #[serde(default)]
    pub assistant: bool,
    /// A project: sessions can work in it, with it mounted at
    /// `/cards/project`. Never also an assistant.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project: bool,
    /// Hidden from agents unless this session has it mounted: never listed,
    /// never mountable, and its cards cannot be linked from another deck.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub private: bool,
    /// Mounted at the start of each of its sessions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<DeckMount>,
    /// Assistant deck ids that may ask it without a prompt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_ask: Vec<String>,
    /// The MCP servers (names in `settings.yaml`) its sessions use, each with
    /// the rules enabling its tools.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub mcp: rumu_core::McpUse,
    /// The rules enabling the skills its sessions see
    /// ([`rumu_core::rules_enable`]; `*` is all, new ones included).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// A project's seal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<DeckIcon>,
}

/// One loaded deck.
#[derive(Debug, Clone)]
pub struct Deck {
    pub meta: DeckMeta,
    /// The deck directory under `<home>/decks/`, named by its id.
    pub dir: PathBuf,
}

impl Deck {
    pub fn repo(&self) -> Repo {
        git::repo(&self.dir)
    }

    pub fn id(&self) -> &str {
        &self.meta.id
    }

    pub fn assistant(&self) -> bool {
        self.meta.assistant
    }

    pub fn project(&self) -> bool {
        self.meta.project
    }

    pub fn private(&self) -> bool {
        self.meta.private
    }

    /// Every card path in the `HEAD` tree, sorted.
    pub fn walk(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for path in self.repo().files()?.keys() {
            let Some(card) = path.strip_suffix(".md") else {
                continue;
            };
            if valid_card_path(card) {
                out.push(card.to_owned());
            }
        }
        Ok(out)
    }

    /// The text of the card at `path` (`<path>.md`) in `HEAD`.
    pub fn read(&self, path: &str) -> Result<Option<String>> {
        if !valid_card_path(path) {
            return Ok(None);
        }
        self.repo().read(&format!("{path}.md"))
    }

    /// The deck's one-line summary: its charter's summary.
    pub fn summary(&self) -> Option<String> {
        let card = parse_card(ABOUT, &self.read(ABOUT).ok()??);
        card.link().is_none().then(|| card.summary()).flatten()
    }

    /// The pinned cards of this deck, in path order. A link is never
    /// pinned: its target's own deck pins it, and a blank card has nothing
    /// to load.
    pub fn pinned(&self) -> Result<Vec<Card>> {
        let mut out = Vec::new();
        for path in self.walk()? {
            if let Some(text) = self.read(&path)? {
                let card = parse_card(&path, &text);
                if card.pinned() && card.link().is_none() && !card.body.trim().is_empty() {
                    out.push(card);
                }
            }
        }
        Ok(out)
    }

    /// The `HEAD` commit, if the repository has one.
    pub fn head(&self) -> Result<Option<String>> {
        self.repo().head()
    }

    /// Commit the new content of the card at `path`.
    pub fn write_file(&self, path: &str, text: &str, author: &Author) -> Result<Card> {
        if !valid_card_path(path) {
            return Err(Error::InvalidPath {
                id: path.to_owned(),
            });
        }
        if frontmatter_is_broken(text) {
            return Err(self.config_err(path, "frontmatter does not parse"));
        }
        let existing = self.read(path)?;
        // Identity: an overwrite keeps the card's id; a new card gets one.
        let id = existing
            .as_deref()
            .and_then(|text| parse_card(path, text).id().map(str::to_owned))
            .or_else(|| parse_card(path, text).id().map(str::to_owned))
            .unwrap_or_else(new_uuid);
        let mut card = parse_card(path, text);
        card.meta.insert("id".into(), serde_json::Value::String(id));
        let text = crate::card::card_content(&card);
        let file = format!("{path}.md");
        let subject = if existing.is_some() {
            format!("update {path}")
        } else {
            format!("create {path}")
        };
        self.commit(
            author,
            &subject,
            &BTreeMap::from([(file, Some(text.clone()))]),
        )?;
        Ok(parse_card(path, &text))
    }

    /// Delete the card at `path` in a commit.
    pub fn remove_file(&self, path: &str, author: &Author) -> Result<()> {
        if !valid_card_path(path) {
            return Err(Error::InvalidPath {
                id: path.to_owned(),
            });
        }
        if self.read(path)?.is_none() {
            return Ok(());
        }
        self.commit(
            author,
            &format!("delete {path}"),
            &BTreeMap::from([(format!("{path}.md"), None)]),
        )?;
        Ok(())
    }

    /// Move the card at `from` to `to` in one commit: a write at the new path
    /// and a removal of the old one. The frontmatter id, and so the usage
    /// score, carries over.
    pub fn move_file(&self, from: &str, to: &str, author: &Author) -> Result<()> {
        if !valid_card_path(from) || !valid_card_path(to) {
            return Err(Error::InvalidPath {
                id: if valid_card_path(from) { to } else { from }.to_owned(),
            });
        }
        if self.read(to)?.is_some() {
            return Err(self.config_err(to, "the destination already exists"));
        }
        let Some(text) = self.read(from)? else {
            return Err(Error::InvalidPath {
                id: from.to_owned(),
            });
        };
        let changes = BTreeMap::from([
            (format!("{from}.md"), None),
            (format!("{to}.md"), Some(text)),
        ]);
        self.commit(author, &format!("move {from} -> {to}"), &changes)?;
        Ok(())
    }

    fn config_err(&self, path: &str, message: &str) -> Error {
        Error::Config {
            path: self.dir.join(path),
            message: message.to_owned(),
        }
    }

    /// Commit one change set with the deck's git identity.
    pub fn commit(
        &self,
        author: &Author,
        subject: &str,
        changes: &BTreeMap<String, Option<String>>,
    ) -> Result<bool> {
        // The removed / replaced cards' identities, read before the commit
        // drops them from `HEAD`.
        let before: BTreeMap<String, String> = changes
            .keys()
            .filter_map(|file| {
                self.repo()
                    .show("HEAD", file)
                    .ok()
                    .flatten()
                    .map(|text| (file.clone(), text))
            })
            .collect();
        let made = self.repo().commit(
            &author.git(),
            subject,
            &git::Trailers {
                changes,
                session: author.session.as_deref(),
                node: author.node.as_deref(),
            },
        )?;
        if made {
            let (written, removed) = changed_ids(changes, &before);
            crate::usage_index::record_commit(&self.dir, &written, &removed, rumu_core::now());
        }
        Ok(made)
    }

    pub fn commits(&self, limit: usize) -> Result<Vec<git::Commit>> {
        self.repo().log(limit)
    }

    pub fn touching(&self, path: &str, since: Option<&str>) -> Result<Vec<git::Commit>> {
        self.repo().log_touching(path, since)
    }

    pub fn text_at(&self, commit: &str, path: &str) -> Result<Option<String>> {
        self.repo().show(commit, path)
    }
}

/// The card ids a commit writes and removes.
fn changed_ids(
    changes: &BTreeMap<String, Option<String>>,
    before: &BTreeMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let mut written = Vec::new();
    let mut removed = Vec::new();
    for (file, content) in changes {
        let Some(path) = file.strip_suffix(".md") else {
            continue;
        };
        if !valid_card_path(path) {
            continue;
        }
        let key = |text: &str| parse_card(path, text).id().map(str::to_owned);
        let old = before.get(file).and_then(|text| key(text));
        let new = content.as_deref().and_then(key);
        if let Some(id) = &new
            && !written.contains(id)
        {
            written.push(id.clone());
        }
        if let Some(id) = old
            && new.as_ref() != Some(&id)
            && !removed.contains(&id)
        {
            removed.push(id);
        }
    }
    (written, removed)
}

/// Who to record as the author of a rumu commit.
#[derive(Debug, Clone)]
pub struct Author {
    /// The session's assistant deck id or `user`.
    pub id: String,
    /// The git author name (deck name or `user`).
    pub name: String,
    pub session: Option<String>,
    pub node: Option<String>,
}

impl Author {
    pub fn user() -> Self {
        Self {
            id: USER.to_owned(),
            name: USER.to_owned(),
            session: None,
            node: None,
        }
    }

    pub fn for_deck(deck: &Deck) -> Self {
        Self {
            id: deck.meta.id.clone(),
            name: deck.meta.name.clone(),
            session: None,
            node: None,
        }
    }

    pub fn git(&self) -> GitAuthor {
        GitAuthor {
            name: self.name.clone(),
            email: format!("{}{}", self.id, git::EMAIL_DOMAIN),
        }
    }
}

/// `grants.json`: action → assistant deck id → the decks it may take that
/// action on without a prompt.
type Grants = BTreeMap<Action, BTreeMap<String, BTreeSet<String>>>;

/// The loaded decks and the default assistant's id.
#[derive(Default)]
struct Inner {
    /// Deck id → deck.
    decks: BTreeMap<String, Deck>,
    default: String,
}

/// The deck registry.
#[derive(Clone)]
pub struct Decks {
    home: PathBuf,
    inner: Arc<RwLock<Inner>>,
    /// Serializes card writes and their commits, so a commit sees the `HEAD`
    /// its write was based on. Grants are read-modify-written under it too.
    write_lock: Arc<Mutex<()>>,
    /// "Allow for this session" answers: (session id, action, deck id).
    session_grants: Arc<Mutex<HashSet<(String, Action, String)>>>,
}

impl Decks {
    /// Open the deck repositories under `RUMU_HOME`. A home with no decks at
    /// all starts with the `user` and `world` decks and a default assistant.
    pub fn load(home: &Path) -> Result<Self> {
        let decks_dir = home.join(DECKS_DIR);
        std::fs::create_dir_all(&decks_dir)
            .map_err(rumu_core::IoError::at("creating", &decks_dir))?;
        let inner = load_home(home)?;
        Ok(Self {
            home: home.to_path_buf(),
            inner: Arc::new(RwLock::new(inner)),
            write_lock: Arc::new(Mutex::new(())),
            session_grants: Arc::default(),
        })
    }

    /// Hold the write lock (see [`Decks::write_lock`]).
    pub(crate) fn lock(&self) -> MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn list(&self) -> Vec<Deck> {
        self.inner.read().unwrap().decks.values().cloned().collect()
    }

    /// The deck id of the default assistant, the one a session runs as when
    /// none is named.
    pub fn default_assistant(&self) -> String {
        self.inner.read().unwrap().default.clone()
    }

    /// Make the assistant deck `id` the default.
    pub fn set_default_assistant(&self, id: &str) -> Result<()> {
        if !self.get(id).is_some_and(|d| d.assistant()) {
            return Err(Error::Config {
                path: self.home.join(DECKS_DIR),
                message: format!("no assistant deck `{id}`"),
            });
        }
        write_default(&self.home, id)?;
        self.inner.write().unwrap().default = id.to_owned();
        Ok(())
    }

    /// Decks listed in the roster.
    pub fn assistants(&self) -> Vec<Deck> {
        self.list()
            .into_iter()
            .filter(|d| d.meta.assistant)
            .collect()
    }

    /// The project decks.
    pub fn projects(&self) -> Vec<Deck> {
        self.list().into_iter().filter(|d| d.meta.project).collect()
    }

    /// The deck with this id.
    pub fn get(&self, id: &str) -> Option<Deck> {
        self.inner.read().unwrap().decks.get(id).cloned()
    }

    /// Is `deck_id` visible to a session running as `assistant` with `mounts`?
    /// A public deck always is. A private deck only through this session's
    /// mount table: mounted on it, the session's own deck, or a `deck.yaml`
    /// mount of the assistant. Only the user puts it there.
    pub fn visible(&self, deck_id: &str, assistant: &str, mounts: &[CardMount]) -> bool {
        let Some(deck) = self.get(deck_id) else {
            return false;
        };
        !deck.private()
            || deck_id == assistant
            || mounts.iter().any(|m| m.deck == deck_id)
            || self
                .get(assistant)
                .is_some_and(|d| d.meta.mounts.iter().any(|m| m.deck == deck_id))
    }

    /// May a card in `from_deck` link to a card in `to_deck`? A private deck
    /// is linked only from within itself: a link shows its target's content,
    /// so cross-deck it would copy that content into every mount of the
    /// other deck.
    pub fn link_allowed(&self, from_deck: &str, to_deck: &str) -> bool {
        from_deck == to_deck || !self.get(to_deck).is_some_and(|d| d.private())
    }

    /// A stored link (`<deck-id>:<path>`) resolved to the card at its target
    /// and the target's deck-local path. `None` when the target is gone or
    /// the link is not allowed (see [`link_allowed`](Self::link_allowed)).
    pub fn link_target(&self, from_deck: &str, stored: &str) -> Option<(String, String, Card)> {
        let (deck_id, path) = stored.split_once(':')?;
        if !self.link_allowed(from_deck, deck_id) {
            return None;
        }
        let deck = self.get(deck_id)?;
        let text = deck.read(path).ok().flatten()?;
        Some((deck_id.to_owned(), path.to_owned(), parse_card(path, &text)))
    }

    /// The default mounts of `deck_id`'s sessions (its `deck.yaml` list).
    /// Entries that cannot be mounted (an unknown deck, a bad or overlapping
    /// mount point, `/cards/self`) are skipped.
    pub fn default_mounts(&self, deck_id: &str) -> Vec<CardMount> {
        let Some(deck) = self.get(deck_id) else {
            return Vec::new();
        };
        let mut out: Vec<CardMount> = Vec::new();
        for m in deck.meta.mounts {
            match self.check_mount(&m, &out) {
                Ok(at) => out.push(CardMount { deck: m.deck, at }),
                Err(message) => {
                    warn!(deck = %deck.meta.name, mount = %m.at, "skipping a default mount: {message}")
                }
            }
        }
        out
    }

    /// Validate one `deck.yaml` mount against the ones before it; the
    /// normalized mount point on success.
    fn check_mount(
        &self,
        m: &DeckMount,
        before: &[CardMount],
    ) -> std::result::Result<String, String> {
        let at = normalize_mount_point(&m.at)?;
        if at == SELF_MOUNT {
            return Err(format!("`{SELF_MOUNT}` is the session's own deck"));
        }
        if at == PROJECT_MOUNT {
            return Err(format!("`{PROJECT_MOUNT}` is the session's project"));
        }
        if self.get(&m.deck).is_none() {
            return Err(format!("no deck with id `{}`", m.deck));
        }
        if let Some(other) = before.iter().find(|o| mount_points_overlap(&o.at, &at)) {
            return Err(format!("`{at}` overlaps `{}`", other.at));
        }
        Ok(at)
    }

    /// May the assistant deck `from` take `action` on `to` in `session`
    /// without asking the user? An assistant may do anything with its own
    /// deck. Otherwise `deck.yaml` grants up front (`from`'s mounts, its
    /// `write: true` mounts, `to`'s `allow_ask`), and so do the user's earlier
    /// answers for the session and for always.
    pub fn allowed(&self, action: Action, session: &str, from: &str, to: &str) -> bool {
        if from == to {
            return true;
        }
        let mounts = |write: bool| {
            self.get(from).is_some_and(|d| {
                d.meta
                    .mounts
                    .iter()
                    .any(|m| m.deck == to && (m.write || !write))
            })
        };
        let configured = match action {
            Action::Mount => mounts(false),
            Action::Write => mounts(true),
            Action::Ask => self
                .get(to)
                .is_some_and(|d| d.meta.allow_ask.iter().any(|a| a == from)),
            Action::Link | Action::Create | Action::Workspace => false,
        };
        configured
            || self.session_grants.lock().unwrap().contains(&(
                session.to_owned(),
                action,
                to.to_owned(),
            ))
            || self
                .grants()
                .get(&action)
                .and_then(|t| t.get(from))
                .is_some_and(|decks| decks.contains(to))
    }

    /// Remember that the user allowed `from` to take `action` on `to`, for
    /// as long as `scope` says.
    pub fn remember(&self, action: Action, scope: Scope, session: &str, from: &str, to: &str) {
        match scope {
            Scope::Once => {}
            Scope::Session => {
                self.session_grants.lock().unwrap().insert((
                    session.to_owned(),
                    action,
                    to.to_owned(),
                ));
            }
            Scope::Always => self.update_grants(|g| {
                g.entry(action)
                    .or_default()
                    .entry(from.to_owned())
                    .or_default()
                    .insert(to.to_owned());
            }),
        }
    }

    /// The grants as currently on disk (the user revokes by editing the file,
    /// so it is read on every check). An unreadable file grants nothing.
    fn grants(&self) -> Grants {
        read_grants(&self.home.join(GRANTS_FILE)).unwrap_or_default()
    }

    /// Read-modify-write `grants.json`. An unreadable file is left alone
    /// rather than replaced, so a typo never wipes the user's grants.
    fn update_grants(&self, change: impl FnOnce(&mut Grants)) {
        let _guard = self.lock();
        let path = self.home.join(GRANTS_FILE);
        let Ok(mut grants) = read_grants(&path) else {
            warn!(path = %path.display(), "not saving a grant: the grants file does not parse");
            return;
        };
        change(&mut grants);
        let text = serde_json::to_string_pretty(&grants).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, text + "\n") {
            warn!(path = %path.display(), error = %e, "saving grants failed");
        }
    }

    /// Does any card in any deck link to `(deck_id, path)`?
    pub fn linked_to(&self, deck_id: &str, path: &str) -> bool {
        self.list().iter().any(|deck| {
            deck.walk().unwrap_or_default().iter().any(|p| {
                deck.read(p)
                    .ok()
                    .flatten()
                    .is_some_and(|text| parse_card(p, &text).link_target() == Some((deck_id, path)))
            })
        })
    }

    /// Is `name` used by a deck other than the one with id `except`? Names
    /// compare case-insensitively, so two decks cannot look alike in a list.
    pub fn name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.trim().to_lowercase();
        self.inner
            .read()
            .unwrap()
            .decks
            .values()
            .any(|d| Some(d.id()) != except && d.meta.name.to_lowercase() == name)
    }

    /// Create (empty `id`) or update a deck's `deck.yaml`. The directory is
    /// named by the id and never moves; a rename only changes the name.
    pub fn upsert(&self, meta: DeckMeta) -> Result<Deck> {
        let mut meta = meta;
        let config_err = |message: String| Error::Config {
            path: self.home.join(DECKS_DIR),
            message,
        };
        meta.name = meta.name.trim().to_owned();
        if !valid_deck_name(&meta.name) {
            return Err(config_err(format!(
                "invalid deck name `{}` (1-{DECK_NAME_MAX} characters, no control characters)",
                meta.name
            )));
        }
        if meta.assistant && meta.project {
            return Err(config_err(format!(
                "deck `{}` cannot be both an assistant and a project",
                meta.name
            )));
        }
        meta.icon = meta.icon.and_then(DeckIcon::normalized);
        if let Some(icon) = &meta.icon {
            icon.check().map_err(config_err)?;
        }
        let mut checked: Vec<CardMount> = Vec::new();
        for m in &meta.mounts {
            let at = self.check_mount(m, &checked).map_err(config_err)?;
            checked.push(CardMount {
                deck: m.deck.clone(),
                at,
            });
        }
        for (m, c) in meta.mounts.iter_mut().zip(checked) {
            m.at = c.at;
        }
        let existing = if meta.id.is_empty() {
            meta.id = new_uuid();
            None
        } else {
            Some(
                self.get(&meta.id)
                    .ok_or_else(|| config_err(format!("no deck with id `{}`", meta.id)))?,
            )
        };
        if self.name_taken(&meta.name, Some(&meta.id)) {
            return Err(config_err(format!(
                "a deck named `{}` already exists",
                meta.name
            )));
        }
        let dir = match &existing {
            Some(deck) => deck.dir.clone(),
            None => self.home.join(DECKS_DIR).join(&meta.id),
        };
        let deck = Deck { meta, dir };
        deck.repo().init()?;
        if existing.is_some() {
            write_deck_meta(&deck, "update deck.yaml", None)?;
        } else {
            write_deck_meta(&deck, "create deck", Some(&Charter::BLANK))?;
        }
        let mut inner = self.inner.write().unwrap();
        inner.decks.insert(deck.meta.id.clone(), deck.clone());
        Ok(deck)
    }

    /// Remove a deck from the roster; cards and history are left in place.
    /// Removing the default assistant moves the default to another one, as
    /// on load (see [`fallback_default`]).
    pub fn remove_assistant(&self, id: &str) -> Result<()> {
        let Some(mut deck) = self.get(id) else {
            return Err(Error::Config {
                path: self.home.join(DECKS_DIR),
                message: format!("unknown deck `{id}`"),
            });
        };
        deck.meta.assistant = false;
        write_deck_meta(&deck, "update deck.yaml", None)?;
        let mut guard = self.inner.write().unwrap();
        let inner = &mut *guard;
        let was_default = inner.default == deck.meta.id;
        inner.decks.insert(deck.meta.id.clone(), deck);
        if was_default {
            inner.default = fallback_default(&self.home, &mut inner.decks)?;
        }
        Ok(())
    }

    /// Delete a deck's directory, its history included. An assistant deck and
    /// a deck another deck mounts are refused: the assistant must be removed,
    /// or the mount dropped, first.
    pub fn delete(&self, id: &str) -> Result<()> {
        let config_err = |message: String| Error::Config {
            path: self.home.join(DECKS_DIR),
            message,
        };
        let deck = self
            .get(id)
            .ok_or_else(|| config_err(format!("unknown deck `{id}`")))?;
        if deck.meta.assistant {
            return Err(config_err(format!(
                "`{}` is an assistant; remove the assistant first",
                deck.meta.name
            )));
        }
        if let Some(by) = self
            .list()
            .into_iter()
            .find(|other| other.meta.mounts.iter().any(|m| m.deck == id))
        {
            return Err(config_err(format!(
                "`{}` is mounted by `{}`",
                deck.meta.name, by.meta.name
            )));
        }
        {
            let _guard = self.lock();
            std::fs::remove_dir_all(&deck.dir)
                .map_err(rumu_core::IoError::at("removing", &deck.dir))?;
        }
        self.inner.write().unwrap().decks.remove(id);
        Ok(())
    }
}

/// Open one deck by its display name under `RUMU_HOME`, without loading the
/// registry.
pub fn open_deck(home: &Path, name: &str) -> Result<Option<Deck>> {
    let decks_dir = home.join(DECKS_DIR);
    let entries = match std::fs::read_dir(&decks_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(rumu_core::IoError::at("reading", &decks_dir)(source).into()),
    };
    for entry in entries.filter_map(|e| e.ok()) {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let dir = entry.path();
        if !dir.join("HEAD").is_file() {
            continue;
        }
        let Some(text) = git::repo(&dir).read(DECK_FILE)? else {
            continue;
        };
        let meta = crate::yaml::from_str::<DeckMeta>(&text).map_err(|e| Error::Config {
            path: dir.join(DECK_FILE),
            message: format!("parsing deck.yaml: {e}"),
        })?;
        if meta.name == name {
            return Ok(Some(Deck { meta, dir }));
        }
    }
    Ok(None)
}

/// `grants.json`: empty when missing, `Err` when present but unreadable.
fn read_grants(path: &Path) -> std::result::Result<Grants, ()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Grants::default()),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cannot read the grants file");
            return Err(());
        }
    };
    serde_json::from_str(&text).map_err(|e| {
        warn!(path = %path.display(), error = %e, "the grants file does not parse; it grants nothing");
    })
}

/// A deck's charter as created with it: a pinned `about.md`.
struct Charter {
    description: &'static str,
    body: &'static str,
}

impl Charter {
    /// A user-created deck's: empty until the user fills it in.
    const BLANK: Charter = Charter {
        description: "",
        body: "",
    };

    /// The file text, with a fresh card id.
    fn text(&self) -> String {
        let mut meta = serde_json::Map::new();
        meta.insert("id".into(), new_uuid().into());
        meta.insert("pinned".into(), true.into());
        card_content(&Card {
            path: ABOUT.to_owned(),
            description: Some(self.description.to_owned()).filter(|d| !d.is_empty()),
            body: self.body.to_owned(),
            revision: String::new(),
            meta,
        })
    }
}

/// Commit `deck.yaml`, and with a new deck its charter, as `user`.
fn write_deck_meta(deck: &Deck, subject: &str, charter: Option<&Charter>) -> Result<()> {
    let text = serde_saphyr::to_string(&deck.meta).map_err(|e| Error::Config {
        path: deck.dir.join(DECK_FILE),
        message: format!("serializing deck.yaml: {e}"),
    })?;
    let mut changes = BTreeMap::from([(DECK_FILE.to_owned(), Some(text))]);
    if let Some(charter) = charter {
        changes.insert(format!("{ABOUT}.md"), Some(charter.text()));
    }
    deck.commit(&Author::user(), subject, &changes)?;
    Ok(())
}

fn read_default(home: &Path) -> Result<Option<String>> {
    let path = home.join(DEFAULT_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text.trim().to_owned()).filter(|id| !id.is_empty())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(rumu_core::IoError::at("reading", &path)(source).into()),
    }
}

fn write_default(home: &Path, id: &str) -> Result<()> {
    let path = home.join(DEFAULT_FILE);
    std::fs::write(&path, format!("{id}\n")).map_err(rumu_core::IoError::at("writing", &path))?;
    Ok(())
}

/// Load `RUMU_HOME`: the decks and the default assistant. When the default
/// file does not name an assistant deck, the first assistant deck (by name)
/// becomes the default; with none, a `default` deck is created.
fn load_home(home: &Path) -> Result<Inner> {
    let decks_dir = home.join(DECKS_DIR);
    std::fs::create_dir_all(&decks_dir).map_err(rumu_core::IoError::at("creating", &decks_dir))?;
    let mut decks = load_decks(&decks_dir)?;
    let saved = read_default(home)?;
    let is_assistant = |id: &str| decks.get(id).is_some_and(|d| d.meta.assistant);
    let default = match saved.filter(|id| is_assistant(id)) {
        Some(id) => id,
        None => fallback_default(home, &mut decks)?,
    };
    Ok(Inner { decks, default })
}

/// A new default assistant, saved as such: the first assistant deck by name,
/// or a new `default` deck when there is none. A home with no decks at all
/// also gets the `user` and `world` decks, mounted by it.
fn fallback_default(home: &Path, decks: &mut BTreeMap<String, Deck>) -> Result<String> {
    let first = decks
        .values()
        .filter(|d| d.meta.assistant)
        .min_by(|a, b| a.meta.name.cmp(&b.meta.name))
        .map(|d| d.meta.id.clone());
    let id = match first {
        Some(id) => id,
        None => {
            let decks_dir = home.join(DECKS_DIR);
            let mut mounts = Vec::new();
            let fresh = decks.is_empty();
            if fresh {
                for (base, charter) in [("user", &USER_CHARTER), ("world", &WORLD_CHARTER)] {
                    let name = free_deck_name(decks, base);
                    let deck = create_deck(&decks_dir, &name, charter, |_| {})?;
                    mounts.push(DeckMount {
                        deck: deck.meta.id.clone(),
                        at: format!("/cards/{name}"),
                        write: true,
                    });
                    decks.insert(deck.meta.id.clone(), deck);
                }
            }
            let name = free_deck_name(decks, "default");
            let deck = create_deck(&decks_dir, &name, &SELF_CHARTER, |meta| {
                meta.assistant = true;
                meta.mounts = mounts;
            })?;
            let id = deck.meta.id.clone();
            decks.insert(id.clone(), deck);
            id
        }
    };
    write_default(home, &id)?;
    Ok(id)
}

/// `base`, or `base-1`, `base-2`, … that no deck in `decks` uses.
fn free_deck_name(decks: &BTreeMap<String, Deck>, base: &str) -> String {
    let taken = |name: &str| {
        decks
            .values()
            .any(|d| d.meta.name.to_lowercase() == name.to_lowercase())
    };
    (0..)
        .map(|i| match i {
            0 => base.to_owned(),
            i => format!("{base}-{i}"),
        })
        .find(|name| !taken(name))
        .expect("some name is free")
}

const SELF_CHARTER: Charter = Charter {
    description: "This assistant: its persona, instructions and what it learned from its own work.",
    body: "Here: who this assistant is and how it should work; what it learned doing its work \
(approaches that worked, mistakes to avoid); and work it has under way.\n\nNot here: facts \
about the user, or facts about tools and the world that hold whoever looks.\n",
};

const USER_CHARTER: Charter = Charter {
    description: "The user: who they are, their preferences, background and habits.",
    body: "Here: facts about the user, whichever assistant learned them. When a preference holds \
only in some context, say so in the card.\n\nNot here: how tools, machines or methods work, or \
how one particular assistant should behave.\n",
};

const WORLD_CHARTER: Charter = Charter {
    description: "Facts and methods that are not about the user and not tied to one project.",
    body: "Here: how tools and software behave, machines and environments, procedures that \
carry across projects.\n\nNot here: the user's own preferences, or the conventions of a single \
project.\n",
};

/// A new deck named `name`, stored at `decks/<id>/`; `init` sets the rest of
/// its `deck.yaml`.
fn create_deck(
    decks_dir: &Path,
    name: &str,
    charter: &Charter,
    init: impl FnOnce(&mut DeckMeta),
) -> Result<Deck> {
    let mut meta = empty_meta(new_uuid(), name.to_owned());
    let dir = decks_dir.join(&meta.id);
    init(&mut meta);
    git::repo(&dir).init()?;
    let deck = Deck { meta, dir };
    write_deck_meta(&deck, "create deck", Some(charter))?;
    Ok(deck)
}

/// Load every bare deck repository under `decks_dir`, newest view of `HEAD`.
fn load_decks(decks_dir: &Path) -> Result<BTreeMap<String, Deck>> {
    let entries =
        std::fs::read_dir(decks_dir).map_err(rumu_core::IoError::at("reading", decks_dir))?;
    let mut dirs: Vec<(String, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let dir = e.file_name().to_str()?.to_owned();
            (!dir.starts_with('.')).then(|| (dir, e.path()))
        })
        .collect();
    dirs.sort_by(|a, b| a.0.cmp(&b.0));

    let mut inner = BTreeMap::new();
    for (dir_name, dir) in dirs {
        let repo = git::repo(&dir);
        let text = match repo.read(DECK_FILE) {
            Ok(Some(text)) => text,
            Ok(None) => {
                warn!(dir = %dir_name, "skipping a directory without deck.yaml in HEAD");
                continue;
            }
            Err(e) => {
                warn!(dir = %dir_name, error = %e, "skipping a directory that is not a bare deck repository");
                continue;
            }
        };
        let meta = match crate::yaml::from_str::<DeckMeta>(&text) {
            Ok(meta) if !meta.id.trim().is_empty() => meta,
            Ok(_) => {
                warn!(dir = %dir_name, "skipping a deck without an id");
                continue;
            }
            Err(e) => {
                warn!(dir = %dir_name, error = %e, "skipping a deck whose deck.yaml does not parse");
                continue;
            }
        };
        if inner.contains_key(&meta.id) {
            warn!(dir = %dir_name, id = %meta.id, "skipping a deck with a duplicate id");
            continue;
        }
        inner.insert(meta.id.clone(), Deck { meta, dir });
    }

    Ok(inner)
}

fn empty_meta(id: String, name: String) -> DeckMeta {
    DeckMeta {
        id,
        name,
        assistant: false,
        project: false,
        private: false,
        mounts: Vec::new(),
        allow_ask: Vec::new(),
        mcp: Default::default(),
        skills: Vec::new(),
        icon: None,
    }
}

#[cfg(test)]
mod tests;
