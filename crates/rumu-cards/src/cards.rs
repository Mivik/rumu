//! The mounted card space: the decks a branch currently sees, each at its
//! mount point under `/cards`.
//!
//! A card's address is `@rumu:/cards/<mount>/<path>` (`/cards/self` is the
//! session's own deck). Only mounted decks take part; link cards resolve to
//! their target across all decks (the link shares the target) and are counted
//! once. A card is identified by its deck id and its path within that deck;
//! links store that pair.

use std::collections::BTreeMap;

use rumu_core::CardMount;

use tracing::warn;

use crate::card::{self, Card, card_content, parse_card, valid_card_path};
use crate::deck::{Author, Deck, Decks};
use crate::{Error, Result};

/// The virtual root of every card address.
pub const CARDS_ROOT: &str = "/cards";
/// Where a session's own deck is mounted.
pub const SELF_MOUNT: &str = "/cards/self";
/// The mount point of the session's project deck.
pub use rumu_core::PROJECT_MOUNT;
/// The prefix a link target is written with.
pub const LINK_PREFIX: &str = "@rumu:/cards/";

/// One segment of a card path: `[a-z][a-z0-9-]*`. Deck names are free-form
/// labels, but the virtual paths that name mount points stay path-safe.
fn valid_path_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Normalize a mount point to `/cards/<name>[/<name>…]` (`user`, `/user` and
/// `/cards/user` are all `/cards/user`). Each segment is path-safe.
pub fn normalize_mount_point(at: &str) -> std::result::Result<String, String> {
    let trimmed = at.trim_matches('/');
    let name = match trimmed {
        "cards" => "",
        _ => trimmed.strip_prefix("cards/").unwrap_or(trimmed),
    };
    if name.is_empty() {
        return Err(
            "a mount point names a directory under `/cards` (for example `/cards/user`)".into(),
        );
    }
    if !name.split('/').all(valid_path_segment) {
        return Err(format!(
            "invalid mount point `/cards/{name}` (each segment follows `[a-z][a-z0-9-]*`)"
        ));
    }
    Ok(format!("{CARDS_ROOT}/{name}"))
}

/// Do two normalized mount points collide (equal, or one inside the other)?
pub fn mount_points_overlap(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// One mounted deck.
#[derive(Clone)]
pub struct Mounted {
    pub mount: CardMount,
    pub deck: Deck,
}

impl Mounted {
    /// The mount point without `/cards/`: `self`, `user`.
    pub fn point(&self) -> &str {
        self.mount
            .at
            .strip_prefix("/cards/")
            .unwrap_or(&self.mount.at)
    }

    /// A public card path: `<point>/<path>`.
    pub fn card_path(&self, path: &str) -> String {
        if path.is_empty() {
            self.point().to_owned()
        } else {
            format!("{}/{}", self.point(), path)
        }
    }
}

/// The branch's mounted card space.
#[derive(Clone)]
pub struct Cards {
    decks: Decks,
    mounts: Vec<Mounted>,
}

/// A card as read through the space: its public path plus content.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Public path (`self/dev/x`) of the address that was read.
    pub path: String,
    pub card: Card,
    /// The deck that owns the content (the target's deck for a link).
    pub deck: String,
    /// The content's path within that deck.
    pub content_path: String,
    /// For a link card, the stored target (`<deck-id>:<path>`).
    pub link: Option<String>,
    /// A link whose target no longer exists (its stored target).
    pub dangling: Option<String>,
}

impl Cards {
    /// Build the space over the mounted decks that still exist.
    pub fn new(decks: &Decks, mounts: &[CardMount]) -> Self {
        let mut resolved: Vec<Mounted> = mounts
            .iter()
            .filter_map(|mount| {
                decks.get(&mount.deck).map(|deck| Mounted {
                    mount: mount.clone(),
                    deck,
                })
            })
            .collect();
        // Longest mount point first; they never overlap, but a stable order
        // keeps output deterministic.
        resolved.sort_by(|a, b| b.mount.at.cmp(&a.mount.at));
        Self {
            decks: decks.clone(),
            mounts: resolved,
        }
    }

    /// The user's space: every deck mounted read-write at `/cards/<id>`.
    /// The library UI reads and writes through it; public paths are
    /// `<id>/<path>`.
    pub fn library(decks: &Decks) -> Self {
        let mounts: Vec<CardMount> = decks
            .list()
            .iter()
            .map(|deck| CardMount {
                deck: deck.id().to_owned(),
                at: format!("{CARDS_ROOT}/{}", deck.id()),
            })
            .collect();
        Self::new(decks, &mounts)
    }

    pub fn mounts(&self) -> &[Mounted] {
        &self.mounts
    }

    /// Is `deck_id` a known deck (mounted or not)?
    pub fn deck_exists(&self, deck_id: &str) -> bool {
        self.decks.get(deck_id).is_some()
    }

    /// The mounted deck ids.
    pub fn deck_ids(&self) -> Vec<String> {
        self.mounts.iter().map(|m| m.deck.id().to_owned()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }

    /// Split a public card path (`self/dev/x`) into its mount and deck path.
    /// `None` unless the path is under a mount and the rest is a valid card
    /// path ([`valid_card_path`]: no `..`, no hidden segments, no bare mount).
    pub fn split<'a>(&'a self, public: &'a str) -> Option<(&'a Mounted, &'a str)> {
        let (mounted, rest) = self.mount_of(public)?;
        valid_card_path(rest).then_some((mounted, rest))
    }

    /// The mount a public path falls under and the unchecked rest.
    pub fn mount_of<'a>(&'a self, public: &'a str) -> Option<(&'a Mounted, &'a str)> {
        let public = public.trim_start_matches('/');
        self.mounts.iter().find_map(|mounted| {
            let rest = public.strip_prefix(mounted.point())?;
            if rest.is_empty() {
                return Some((mounted, rest));
            }
            rest.strip_prefix('/').map(|rest| (mounted, rest))
        })
    }

    /// The public path of a card in `deck_id` (its first mount point).
    pub fn public_path(&self, deck_id: &str, path: &str) -> Option<String> {
        self.mounts
            .iter()
            .find(|m| m.deck.id() == deck_id)
            .map(|m| m.card_path(path))
    }

    /// How a card in `deck_id` is shown to the model: its mount path, or
    /// `<deck-name>:<path>` when its deck is not mounted.
    pub fn display_path(&self, deck_id: &str, path: &str) -> String {
        if let Some(public) = self.public_path(deck_id, path) {
            return public;
        }
        let name = self
            .decks
            .get(deck_id)
            .map(|d| d.meta.name)
            .unwrap_or_else(|| deck_id.to_owned());
        format!("{name}:{path}")
    }

    /// Every raw card path, public, sorted.
    pub fn walk(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for mounted in &self.mounts {
            for path in mounted.deck.walk()? {
                out.push(mounted.card_path(&path));
            }
        }
        out.sort();
        Ok(out)
    }

    /// Resolve a model-written link target (`@rumu:/cards/<path>`) to the
    /// target's deck id and path. The path must be one of the mounted card
    /// paths.
    pub fn resolve_link(&self, value: &str) -> Result<(String, String)> {
        let Some(path) = value.trim().strip_prefix(LINK_PREFIX) else {
            return Err(Error::Config {
                path: "card link".into(),
                message: format!("a link target is `{LINK_PREFIX}<path>` (got `{value}`)"),
            });
        };
        let Some((mounted, raw)) = self.split(path) else {
            return Err(Error::Config {
                path: "card link".into(),
                message: format!("no card at `{value}`"),
            });
        };
        if mounted.deck.read(raw)?.is_none() {
            return Err(Error::Config {
                path: "card link".into(),
                message: format!("no card at `{value}`"),
            });
        }
        Ok((mounted.deck.id().to_owned(), raw.to_owned()))
    }

    /// Load one public card path, following a link one level.
    pub fn load(&self, public: &str) -> Result<Option<Loaded>> {
        let Some((mounted, path)) = self.split(public) else {
            return Ok(None);
        };
        let Some(text) = mounted.deck.read(path)? else {
            return Ok(None);
        };
        let raw = parse_card(path, &text);
        let public = mounted.card_path(path);
        match raw.link().map(str::to_owned) {
            Some(stored) => {
                if let Some((deck_id, target_path, target_card)) =
                    self.decks.link_target(mounted.deck.id(), &stored)
                {
                    let display = self.display_path(&deck_id, &target_path);
                    Ok(Some(Loaded {
                        path: public,
                        card: Card {
                            path: display,
                            ..target_card
                        },
                        deck: deck_id,
                        content_path: target_path,
                        link: Some(stored),
                        dangling: None,
                    }))
                } else {
                    Ok(Some(Loaded {
                        path: public.clone(),
                        card: Card {
                            path: public,
                            ..raw
                        },
                        deck: mounted.deck.id().to_owned(),
                        content_path: path.to_owned(),
                        link: None,
                        dangling: Some(stored),
                    }))
                }
            }
            None => Ok(Some(Loaded {
                path: public.clone(),
                card: Card {
                    path: public,
                    ..raw
                },
                deck: mounted.deck.id().to_owned(),
                content_path: path.to_owned(),
                link: None,
                dangling: None,
            })),
        }
    }

    /// Every visible card, keyed by public path, deduplicated by identity
    /// (deck id + path). A direct card wins over a link to it; a dangling
    /// link is included with its own content and marked.
    pub fn all(&self) -> Result<BTreeMap<String, Loaded>> {
        let mut by_key: BTreeMap<(String, String), Loaded> = BTreeMap::new();
        let mut dangling: Vec<Loaded> = Vec::new();
        for mounted in &self.mounts {
            for path in mounted.deck.walk()? {
                let text = match mounted.deck.read(&path) {
                    Ok(Some(text)) => text,
                    Ok(None) => continue,
                    Err(e) => {
                        warn!(deck = %mounted.deck.meta.name, path = %path, error = %e, "skipping an unreadable card");
                        continue;
                    }
                };
                let raw = parse_card(&path, &text);
                let public = mounted.card_path(&path);
                match raw.link().map(str::to_owned) {
                    Some(stored) => {
                        if let Some((deck_id, target_path, card)) =
                            self.decks.link_target(mounted.deck.id(), &stored)
                        {
                            let display = self.display_path(&deck_id, &target_path);
                            let loaded = Loaded {
                                path: public,
                                card: Card {
                                    path: display,
                                    ..card
                                },
                                deck: deck_id,
                                content_path: target_path.clone(),
                                link: Some(stored),
                                dangling: None,
                            };
                            match by_key.get(&(loaded.deck.clone(), target_path)) {
                                Some(existing) if existing.link.is_none() => {}
                                _ => {
                                    by_key.insert(
                                        (loaded.deck.clone(), loaded.content_path.clone()),
                                        loaded,
                                    );
                                }
                            }
                        } else {
                            dangling.push(Loaded {
                                path: public.clone(),
                                card: Card {
                                    path: public,
                                    ..raw
                                },
                                deck: mounted.deck.id().to_owned(),
                                content_path: path.to_owned(),
                                link: None,
                                dangling: Some(stored),
                            });
                        }
                    }
                    None => {
                        let loaded = Loaded {
                            path: public.clone(),
                            card: Card {
                                path: public,
                                ..raw
                            },
                            deck: mounted.deck.id().to_owned(),
                            content_path: path.to_owned(),
                            link: None,
                            dangling: None,
                        };
                        by_key.insert((loaded.deck.clone(), loaded.content_path.clone()), loaded);
                    }
                }
            }
        }
        let mut out = BTreeMap::new();
        for loaded in by_key.into_values() {
            out.insert(loaded.path.clone(), loaded);
        }
        for loaded in dangling {
            out.insert(loaded.path.clone(), loaded);
        }
        Ok(out)
    }

    /// The cards whose text recall/search ranks: display path → card.
    pub fn rankable(&self) -> Result<BTreeMap<String, Card>> {
        Ok(self
            .all()?
            .into_values()
            .filter(|l| l.dangling.is_none())
            .map(|l| (l.card.path.clone(), l.card))
            .collect())
    }

    /// The identity (deck id + deck-local path) of every live card, by the
    /// same display path [`rankable`](Self::rankable) uses.
    pub fn identities(&self) -> Result<BTreeMap<String, (String, String)>> {
        Ok(self
            .all()?
            .into_values()
            .filter(|l| l.dangling.is_none())
            .map(|l| (l.card.path, (l.deck, l.content_path)))
            .collect())
    }

    /// Write (or create or move) a card through a public path. Writes through
    /// link cards are refused. Each write is one git commit.
    ///
    /// `expect_view_rev` is the CAS base of the card the write replaces: a
    /// move's source, else the destination (`""`: it must not exist yet). When
    /// given, that card's current editable view must still hash to it. It is
    /// checked under the write lock, so two writes based on the same read
    /// cannot both land.
    pub fn write(
        &self,
        public: &str,
        view: &str,
        previous: Option<&str>,
        expect_view_rev: Option<&str>,
        author: &Author,
    ) -> Result<Card> {
        let Some((mounted, path)) = self.split(public) else {
            return Err(Error::InvalidPath {
                id: public.to_owned(),
            });
        };
        let _guard = self.decks.lock();
        let existing = mounted.deck.read(path)?.map(|text| parse_card(path, &text));
        // A move's source (`previous` naming another card), read under the
        // lock like the destination.
        let source = match previous.filter(|prev| *prev != public) {
            Some(prev) => {
                let missing = || Error::InvalidPath {
                    id: prev.to_owned(),
                };
                let (m, p) = self.split(prev).ok_or_else(missing)?;
                let text = m.deck.read(p)?.ok_or_else(missing)?;
                Some((prev, m, p, parse_card(p, &text)))
            }
            None => None,
        };
        if let Some(expected) = expect_view_rev {
            let (checked, replaced) = match &source {
                Some((prev, _, _, raw)) => (*prev, Some(raw)),
                None => (public, existing.as_ref()),
            };
            let current = replaced.map(|c| card::view_rev_of(&card::view_of(c)));
            if current.as_deref().unwrap_or_default() != expected {
                return Err(Error::Stale {
                    id: checked.to_owned(),
                });
            }
        }
        if source.is_some() && existing.is_some() {
            return Err(Error::Config {
                path: "card view".into(),
                message: format!("the move destination `{public}` already exists"),
            });
        }
        if existing.as_ref().is_some_and(|c| c.link().is_some()) {
            return Err(Error::LinkWrite {
                id: public.to_owned(),
            });
        }
        if let Some((prev, _, _, raw)) = &source
            && raw.link().is_some()
        {
            return Err(Error::LinkWrite {
                id: (*prev).to_owned(),
            });
        }
        let mut card = card::card_from_view(path, view).map_err(|e| Error::Config {
            path: "card view".into(),
            message: e.to_string(),
        })?;
        // A link stores the target's deck id and path; the model only ever
        // writes paths.
        if let Some(target) = card.link().map(str::to_owned) {
            let (target_deck, target_path) = self.resolve_link(&target)?;
            if target_deck == mounted.deck.id() && target_path == path {
                return Err(Error::Config {
                    path: "card view".into(),
                    message: "a card cannot link to itself".into(),
                });
            }
            if !self.decks.link_allowed(mounted.deck.id(), &target_deck) {
                return Err(Error::Config {
                    path: "card view".into(),
                    message: "a card in a private deck can only be linked from its own deck".into(),
                });
            }
            if let Some(target_card) = self
                .decks
                .get(&target_deck)
                .and_then(|deck| deck.read(&target_path).ok().flatten())
                .map(|text| parse_card(&target_path, &text))
                && target_card.link().is_some()
            {
                return Err(Error::LinkToLink { target });
            }
            // Turning a card into a link must not make the links to it
            // two levels deep.
            if self.decks.linked_to(mounted.deck.id(), path) {
                return Err(Error::LinkedTarget {
                    id: public.to_owned(),
                });
            }
            card.meta.insert(
                "link".into(),
                serde_json::Value::String(format!("{target_deck}:{target_path}")),
            );
        }
        // Identity: an overwrite keeps the card's id and a move carries it to
        // the destination; a new card gets a fresh one. A model-written `id:`
        // in the view is ignored, since `card_from_view` cannot see one.
        let id = existing
            .as_ref()
            .and_then(Card::id)
            .or_else(|| source.as_ref().and_then(|(.., raw)| raw.id()))
            .map(str::to_owned)
            .unwrap_or_else(card::new_uuid);
        card.meta.insert("id".into(), serde_json::Value::String(id));
        let content = card_content(&card);
        match &source {
            // Cross-deck move: commit the destination first, then the source
            // removal.
            Some((_, prev_mount, prev_path, _)) if prev_mount.deck.id() != mounted.deck.id() => {
                mounted.deck.commit(
                    author,
                    &format!("move {prev_path} -> {path}"),
                    &BTreeMap::from([(format!("{path}.md"), Some(content))]),
                )?;
                prev_mount.deck.commit(
                    author,
                    &format!("delete {prev_path}"),
                    &BTreeMap::from([(format!("{prev_path}.md"), None)]),
                )?;
            }
            _ => {
                let previous_path = source.as_ref().map(|(_, _, p, _)| *p);
                let subject = if card.link().is_some() {
                    format!("link {path}")
                } else if let Some(old) = previous_path {
                    format!("move {old} -> {path}")
                } else if existing.is_some() {
                    format!("update {path}")
                } else {
                    format!("create {path}")
                };
                let mut changes = BTreeMap::from([(format!("{path}.md"), Some(content))]);
                if let Some(old) = previous_path {
                    changes.insert(format!("{old}.md"), None);
                }
                mounted.deck.commit(author, &subject, &changes)?;
            }
        }
        let public = mounted.card_path(path);
        Ok(Card {
            path: public,
            ..card
        })
    }

    /// Delete the card at a public path (the file itself; a link card is
    /// removed, not its target). `expect_view_rev` is the CAS base as in
    /// [`Cards::write`]. One commit, `delete <path>`.
    pub fn remove(
        &self,
        public: &str,
        expect_view_rev: Option<&str>,
        author: &Author,
    ) -> Result<()> {
        let Some((mounted, path)) = self.split(public) else {
            return Err(Error::InvalidPath {
                id: public.to_owned(),
            });
        };
        let _guard = self.decks.lock();
        let Some(text) = mounted.deck.read(path)? else {
            return Err(Error::InvalidPath {
                id: public.to_owned(),
            });
        };
        let raw = parse_card(path, &text);
        if let Some(expected) = expect_view_rev {
            let current = card::view_rev_of(&card::view_of(&raw));
            if current != expected {
                return Err(Error::Stale {
                    id: public.to_owned(),
                });
            }
        }
        mounted.deck.commit(
            author,
            &format!("delete {path}"),
            &BTreeMap::from([(format!("{path}.md"), None)]),
        )?;
        Ok(())
    }

    /// The current raw file text of a public card path.
    pub fn current_text(&self, public: &str) -> Result<Option<String>> {
        let Some((mounted, path)) = self.split(public) else {
            return Ok(None);
        };
        mounted.deck.read(path)
    }

    /// The `HEAD` commit of the deck holding a public card path.
    pub fn head(&self, public: &str) -> Result<Option<String>> {
        let Some((mounted, _)) = self.split(public) else {
            return Ok(None);
        };
        mounted.deck.head()
    }

    /// The deck holding a public card path.
    pub fn deck_for(&self, public: &str) -> Option<Deck> {
        self.split(public).map(|(m, _)| m.deck.clone())
    }

    /// The raw (unresolved) card at a public path.
    pub fn raw(&self, public: &str) -> Result<Option<Card>> {
        let Some((mounted, path)) = self.split(public) else {
            return Ok(None);
        };
        Ok(mounted.deck.read(path)?.map(|text| parse_card(path, &text)))
    }
}
