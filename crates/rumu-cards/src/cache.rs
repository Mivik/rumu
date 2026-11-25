//! Each deck's parsed cards and their recall [`Index`], cached by the deck's
//! `HEAD` commit. A new `HEAD` re-reads and re-tokenizes only the cards whose
//! blob changed; everything else carries over. The cache is derived data:
//! nothing in it is written anywhere.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use tracing::warn;

use crate::Result;
use crate::card::{Card, parse_card, valid_card_path};
use crate::deck::Deck;
use crate::recall::{CARD_WEIGHTS, Doc, Index};

/// One card of a deck's `HEAD`.
pub struct Entry {
    /// The deck-local path (no `.md`).
    pub path: String,
    oid: String,
    /// Parsed as stored: a link card is its link, not its target.
    pub card: Arc<Card>,
}

/// A deck's cards at one `HEAD`, by path, and their index (document `i` is
/// entry `i`; a link card's is empty, as it ranks as its target).
pub struct DeckCards {
    head: Option<String>,
    entries: Vec<Entry>,
    index: Index,
}

impl DeckCards {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry at `path`, with its position.
    pub fn find(&self, path: &str) -> Option<(usize, &Entry)> {
        let i = self
            .entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()?;
        Some((i, &self.entries[i]))
    }

    pub fn index(&self) -> &Index {
        &self.index
    }
}

/// Deck directory → its cards at the `HEAD` last seen.
static CACHE: LazyLock<Mutex<HashMap<PathBuf, Arc<DeckCards>>>> = LazyLock::new(Mutex::default);

/// The deck's cards at its current `HEAD`.
pub fn load(deck: &Deck) -> Result<Arc<DeckCards>> {
    let repo = deck.repo();
    let head = repo.head_id()?;
    let old = CACHE.lock().unwrap().get(&deck.dir).cloned();
    if let Some(old) = old.as_ref().filter(|old| old.head == head) {
        return Ok(old.clone());
    }
    // Built outside the lock; a concurrent build of the same deck is only
    // wasted work.
    let (head, files) = repo.head_files()?;
    let empty = DeckCards {
        head: None,
        entries: Vec::new(),
        index: Index::new(CARD_WEIGHTS, []),
    };
    let old = old.unwrap_or_else(|| Arc::new(empty));
    let mut listed: Vec<(String, String)> = files
        .into_iter()
        .filter_map(|(file, oid)| {
            let path = file.strip_suffix(".md").filter(|p| valid_card_path(p))?;
            Some((path.to_owned(), oid))
        })
        .collect();
    listed.sort();
    // Unchanged blobs keep their card and their postings.
    let mut kept: Vec<Option<u32>> = vec![None; old.entries.len()];
    let mut entries: Vec<Option<Entry>> = Vec::with_capacity(listed.len());
    let mut unread: Vec<usize> = Vec::new();
    for (j, (path, oid)) in listed.iter().enumerate() {
        match old.find(path).filter(|(_, e)| e.oid == *oid) {
            Some((i, e)) => {
                kept[i] = Some(j as u32);
                entries.push(Some(Entry {
                    path: e.path.clone(),
                    oid: e.oid.clone(),
                    card: e.card.clone(),
                }));
            }
            None => {
                entries.push(None);
                unread.push(j);
            }
        }
    }
    let oids: Vec<&str> = unread.iter().map(|&j| listed[j].1.as_str()).collect();
    let mut added: Vec<(u32, Doc)> = Vec::new();
    for (&j, text) in unread.iter().zip(repo.blobs(&oids)?) {
        let (path, oid) = &listed[j];
        let card = match text {
            Ok(text) => parse_card(path, &text),
            Err(e) => {
                warn!(deck = %deck.meta.name, path = %path, error = %e, "skipping an unreadable card");
                continue;
            }
        };
        if card.link().is_none() {
            added.push((j as u32, Doc::card(&card, path)));
        }
        entries[j] = Some(Entry {
            path: path.clone(),
            oid: oid.clone(),
            card: Arc::new(card),
        });
    }
    // Number the entries that were read; an unreadable card has none.
    let mut number = vec![None; entries.len()];
    let mut n = 0;
    for (j, e) in entries.iter().enumerate() {
        if e.is_some() {
            number[j] = Some(n);
            n += 1;
        }
    }
    let kept: Vec<Option<u32>> = kept
        .iter()
        .map(|j| j.and_then(|j| number[j as usize]))
        .collect();
    let added = added
        .iter()
        .filter_map(|(j, doc)| Some((number[*j as usize]?, doc)));
    let index = old.index.update(&kept, n as usize, added);
    let cards = Arc::new(DeckCards {
        head,
        entries: entries.into_iter().flatten().collect(),
        index,
    });
    CACHE
        .lock()
        .unwrap()
        .insert(deck.dir.clone(), cards.clone());
    Ok(cards)
}

/// Drop a deck's cache (its directory is gone).
pub fn forget(dir: &Path) {
    CACHE.lock().unwrap().remove(dir);
}
