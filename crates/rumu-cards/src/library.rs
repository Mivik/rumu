//! The card library: the user's view of every deck, independent of any
//! assistant's mounts. The web Library page reads and writes through these
//! functions; the wire always uses `{deck, path}` and the deck id is only
//! ever an internal address in [`Cards::library`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::card::{self, Card, parse_card, valid_card_path};
use crate::cards::Cards;
use crate::deck::{Author, Deck, DeckMeta, Decks};
use crate::diff::{self, Hunk};
use crate::recall;
use crate::{Error, Result};

/// One deck in the library sidebar.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryDeck {
    pub id: String,
    /// The display name (`deck.yaml` `name`).
    pub name: String,
    pub assistant: bool,
    pub project: bool,
    /// The one-line summary from its charter (`about.md`).
    pub description: Option<String>,
    /// The number of card files in the deck.
    pub cards: usize,
}

/// A card target as `{deck id, path within the deck}`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryRef {
    pub deck: String,
    pub path: String,
}

/// One card row in the library list (no body).
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryCard {
    /// The deck id.
    pub deck: String,
    /// The path within the deck, without `.md`.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub pinned: bool,
    /// A live link card's target; `None` for an ordinary card and a dangling
    /// link (see `dangling`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<LibraryRef>,
    /// A link whose target no longer exists.
    pub dangling: bool,
    /// Only on search results: a short body snippet around the first match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

/// One card opened in the editor.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryCardDetail {
    pub card: LibraryCard,
    /// The editable view (frontmatter, then body) — what a save sends back.
    pub view: String,
    /// CAS base for save / delete.
    pub view_rev: String,
    /// Other cards link to this one (deleting it leaves them dangling).
    pub linked: bool,
    /// For a link card: the target's editable view, shown read-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_view: Option<String>,
}

/// One version in a card's history.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryCommit {
    pub commit: String,
    pub short: String,
    pub author: String,
    pub at: i64,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The card's path (within the deck) at this commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One commit's version of a card and its diff from the previous version.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct LibraryVersion {
    /// The card's editable view at the commit; `None` for a deletion.
    pub view: Option<String>,
    /// The same for the next-older commit in this deck, if any.
    pub previous: Option<String>,
    pub hunks: Vec<Hunk>,
}

/// The request of [`save`]. `from` is the card's previous location when the
/// user changed its path or deck (a move).
#[derive(Debug, Clone, Copy)]
pub struct SaveRequest<'a> {
    pub path: &'a str,
    pub view: &'a str,
    pub base: &'a str,
    pub from: Option<(&'a Deck, &'a str)>,
}

/// A failure that names what could not be found.
fn not_found(what: String) -> Error {
    Error::Config {
        path: PathBuf::from("library"),
        message: what,
    }
}

/// The library space (`/cards/<id>`) of every deck.
fn library_cards(decks: &Decks) -> Cards {
    Cards::library(decks)
}

/// A deck's public library path for one card: `<id>/<path>`.
fn public_path(deck: &Deck, path: &str) -> String {
    format!("{}/{}", deck.id(), path)
}

/// The sidebar's decks, sorted by display name.
pub fn decks(decks: &Decks) -> Result<Vec<LibraryDeck>> {
    let mut out = Vec::new();
    for deck in decks.list() {
        out.push(library_deck(&deck)?);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn library_deck(deck: &Deck) -> Result<LibraryDeck> {
    Ok(LibraryDeck {
        id: deck.id().to_owned(),
        name: deck.meta.name.clone(),
        assistant: deck.meta.assistant,
        project: deck.meta.project,
        description: deck.summary(),
        cards: deck.walk()?.len(),
    })
}

/// Create a deck with a display name.
pub fn deck_create(decks: &Decks, name: &str) -> Result<LibraryDeck> {
    let deck = decks.upsert(DeckMeta {
        id: String::new(),
        name: name.to_owned(),
        assistant: false,
        project: false,
        private: false,
        mounts: Vec::new(),
        allow_ask: Vec::new(),
        model: None,
        mcp: Default::default(),
        skills: Vec::new(),
        icon: None,
        cat: None,
    })?;
    library_deck(&deck)
}

/// Rename a deck (only its display name; its id, directory and cards stay).
pub fn deck_rename(decks: &Decks, deck: &Deck, name: &str) -> Result<LibraryDeck> {
    let mut meta = deck.meta.clone();
    meta.name = name.to_owned();
    library_deck(&decks.upsert(meta)?)
}

/// The rows of one deck, or of every deck when `deck` is `None`, sorted by
/// deck display name then path.
pub fn cards(decks: &Decks, deck: Option<&Deck>) -> Result<Vec<LibraryCard>> {
    let cards = library_cards(decks);
    let mut out = Vec::new();
    for public in cards.walk()? {
        let Some((mounted, path)) = cards.mount_of(&public) else {
            continue;
        };
        if deck.is_some_and(|deck| mounted.deck.id() != deck.id()) {
            continue;
        }
        let Some(raw) = cards.raw(&public)? else {
            continue;
        };
        out.push(card_row(decks, &mounted.deck, path, &raw)?);
    }
    sort_rows(decks, &mut out);
    Ok(out)
}

/// Search one deck, or every deck when `deck` is `None`, best match first
/// (at most 50), with a body snippet per hit.
pub fn search(decks: &Decks, query: &str, deck: Option<&Deck>) -> Result<Vec<LibraryCard>> {
    let query = query.trim();
    if query.is_empty() {
        return Err(not_found("missing `query`".to_owned()));
    }
    let cards = library_cards(decks);
    let loaded = cards.all()?;
    let selected = |public: &str| match deck {
        Some(deck) => public.starts_with(&format!("{}/", deck.id())),
        None => true,
    };
    let rankable: BTreeMap<String, Card> = loaded
        .iter()
        .filter(|(public, l)| selected(public) && l.dangling.is_none())
        .map(|(_, l)| (l.card.path.clone(), l.card.clone()))
        .collect();
    let address: BTreeMap<&str, &str> = loaded
        .iter()
        .filter(|(public, _)| selected(public))
        .map(|(public, l)| (l.card.path.as_str(), public.as_str()))
        .collect();
    let terms = recall::tokens(query);
    let mut out = Vec::new();
    for hit in recall::search(&rankable, query, 50) {
        let Some(public) = address.get(hit.card.path.as_str()) else {
            continue;
        };
        let Some((mounted, path)) = cards.mount_of(public) else {
            continue;
        };
        let Some(raw) = cards.raw(public)? else {
            continue;
        };
        let mut row = card_row(decks, &mounted.deck, path, &raw)?;
        row.snippet = crate::body_snippet(&hit.card.body, &terms);
        out.push(row);
    }
    Ok(out)
}

/// One card with its editable view and CAS base.
pub fn detail(decks: &Decks, deck: &Deck, path: &str) -> Result<LibraryCardDetail> {
    let cards = library_cards(decks);
    let public = public_path(deck, path);
    let Some(raw) = cards.raw(&public)? else {
        return Err(not_found(format!("no card at `{public}`")));
    };
    let view = card::view_of(&raw);
    let view_rev = card::view_rev_of(&view);
    let linked = decks.linked_to(deck.id(), path);
    let target_view = raw.link().and_then(|stored| {
        let (_, _, card) = decks.link_target(deck.id(), stored)?;
        Some(card::view_of(&card))
    });
    Ok(LibraryCardDetail {
        card: card_row(decks, deck, path, &raw)?,
        view,
        view_rev,
        linked,
        target_view,
    })
}

/// Write a card (create, update or move) as `user`, then return its detail.
///
/// The CAS base is the opened card's: the destination's, or for a move the
/// source's (the destination must not exist).
pub fn save(decks: &Decks, deck: &Deck, req: SaveRequest<'_>) -> Result<LibraryCardDetail> {
    if !valid_card_path(req.path) {
        return Err(Error::Config {
            path: PathBuf::from("card path"),
            message: format!(
                "invalid card path `{}` (no `..`, no hidden segments)",
                req.path
            ),
        });
    }
    let public = public_path(deck, req.path);
    let previous = req
        .from
        .map(|(from_deck, from_path)| public_path(from_deck, from_path));
    library_cards(decks).write(
        &public,
        req.view,
        previous.as_deref(),
        Some(req.base),
        &Author::user(),
    )?;
    detail(decks, deck, req.path)
}

/// Delete a card as `user` (a link card removes the link file, not its
/// target). `base` is the `view_rev` the editor opened.
pub fn delete(decks: &Decks, deck: &Deck, path: &str, base: &str) -> Result<()> {
    library_cards(decks).remove(&public_path(deck, path), Some(base), &Author::user())
}

/// The card paths a card or folder operation covers: the card itself, or
/// every card under the folder. Empty when nothing is there.
fn covered(deck: &Deck, path: &str, folder: bool) -> Result<Vec<String>> {
    let walked = deck.walk()?;
    Ok(if folder {
        let prefix = format!("{path}/");
        walked
            .into_iter()
            .filter(|p| p.starts_with(&prefix))
            .collect()
    } else {
        walked.into_iter().filter(|p| p == path).collect()
    })
}

fn invalid_path(path: &str) -> Error {
    Error::Config {
        path: PathBuf::from("card path"),
        message: format!("invalid path `{path}` (no `..`, no hidden segments)"),
    }
}

/// Rename or move a card, or a whole folder (`folder`), within one deck as
/// `user`, in one commit. Nothing carries over: links and use records stay
/// with the old path. No destination may exist yet.
pub fn move_path(decks: &Decks, deck: &Deck, from: &str, to: &str, folder: bool) -> Result<()> {
    for path in [from, to] {
        if !valid_card_path(path) {
            return Err(invalid_path(path));
        }
    }
    if from == to {
        return Ok(());
    }
    if folder && to.starts_with(&format!("{from}/")) {
        return Err(not_found(format!("cannot move `{from}` into itself")));
    }
    let _guard = decks.lock();
    let sources = covered(deck, from, folder)?;
    if sources.is_empty() {
        return Err(not_found(format!(
            "nothing at `{}`",
            public_path(deck, from)
        )));
    }
    let target = |source: &str| format!("{to}{}", &source[from.len()..]);
    for source in &sources {
        let dest = target(source);
        if deck.read(&dest)?.is_some() {
            return Err(not_found(format!(
                "`{}` already exists",
                public_path(deck, &dest)
            )));
        }
    }
    let mut changes = BTreeMap::new();
    for source in &sources {
        let dest = target(source);
        let Some(text) = deck.read(source)? else {
            continue;
        };
        changes.insert(format!("{source}.md"), None);
        changes.insert(format!("{dest}.md"), Some(text));
    }
    deck.commit(&Author::user(), &format!("move {from} -> {to}"), &changes)?;
    Ok(())
}

/// Delete every card under a folder as `user`, in one commit. Links to them
/// are left dangling.
pub fn delete_folder(decks: &Decks, deck: &Deck, path: &str) -> Result<()> {
    if !valid_card_path(path) {
        return Err(invalid_path(path));
    }
    let _guard = decks.lock();
    let sources = covered(deck, path, true)?;
    if sources.is_empty() {
        return Err(not_found(format!(
            "nothing at `{}`",
            public_path(deck, path)
        )));
    }
    let mut changes = BTreeMap::new();
    for source in &sources {
        changes.insert(format!("{source}.md"), None);
    }
    deck.commit(&Author::user(), &format!("delete {path}/"), &changes)?;
    Ok(())
}

/// A card path's commits in one deck, newest first.
pub fn history(_decks: &Decks, deck: &Deck, path: &str) -> Result<Vec<LibraryCommit>> {
    Ok(deck
        .touching(path, None)?
        .into_iter()
        .map(|commit| LibraryCommit {
            commit: commit.hash,
            short: commit.short,
            author: commit.author,
            at: commit.at,
            subject: commit.subject,
            session: commit.session,
            path: Some(path.to_owned()),
        })
        .collect())
}

/// One commit's version of a card and its diff from the previous version at
/// the same path.
pub fn version(_decks: &Decks, deck: &Deck, path: &str, commit: &str) -> Result<LibraryVersion> {
    let commits = deck.touching(path, None)?;
    let Some(index) = commits.iter().position(|c| c.hash == commit) else {
        return Err(not_found(format!(
            "commit `{commit}` does not change `{path}`"
        )));
    };
    let file = format!("{path}.md");
    let view = deck
        .text_at(commit, &file)?
        .map(|text| card::view_of(&parse_card(path, &text)));
    let previous = commits.get(index + 1).and_then(|previous| {
        let text = deck.text_at(&previous.hash, &file).ok().flatten()?;
        Some(card::view_of(&parse_card(path, &text)))
    });
    let hunks = diff::hunks(
        previous.as_deref().unwrap_or_default(),
        view.as_deref().unwrap_or_default(),
    );
    Ok(LibraryVersion {
        view,
        previous,
        hunks,
    })
}

/// Turn one raw card file into a row. A link is the link file itself with
/// its target's content; `link` is that target, or `dangling` is set.
fn card_row(decks: &Decks, deck: &Deck, path: &str, raw: &Card) -> Result<LibraryCard> {
    let target = raw.link().and_then(|stored| {
        let (deck, path, card) = decks.link_target(deck.id(), stored)?;
        Some((LibraryRef { deck, path }, card))
    });
    let dangling = raw.link().is_some() && target.is_none();
    let shown = target.as_ref().map_or(raw, |(_, card)| card);
    Ok(LibraryCard {
        deck: deck.id().to_owned(),
        path: path.to_owned(),
        description: shown.description.clone(),
        keywords: shown.keywords().into_iter().map(str::to_owned).collect(),
        pinned: shown.pinned(),
        link: target.map(|(at, _)| at),
        dangling,
        snippet: None,
    })
}

/// Sort rows by deck display name, then path.
fn sort_rows(decks: &Decks, rows: &mut [LibraryCard]) {
    let names: BTreeMap<String, String> = decks
        .list()
        .into_iter()
        .map(|deck| (deck.meta.id, deck.meta.name))
        .collect();
    rows.sort_by(|a, b| {
        names
            .get(&a.deck)
            .cmp(&names.get(&b.deck))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests;
