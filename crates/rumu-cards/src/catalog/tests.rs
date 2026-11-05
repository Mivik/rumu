//! Per-deck catalog tests: budget selection, path order and folding.

use std::path::Path;

use crate::card::parse_card;
use crate::deck::{Author, Deck, DeckMeta, Decks};
use crate::usage_index;

use super::*;

fn deck_with(home: &Path, paths: &[String]) -> (Decks, Deck) {
    let decks = Decks::load(home).unwrap();
    let deck = decks
        .upsert(DeckMeta {
            id: String::new(),
            name: "main".into(),
            assistant: false,
            workspace: None,
            mounts: Vec::new(),
            allow_ask: Vec::new(),
        })
        .unwrap();
    for path in paths {
        deck.write_file(
            path,
            &format!("---\ndescription: {path}\n---\nbody\n"),
            &Author::user(),
        )
        .unwrap();
    }
    (decks, deck)
}

fn id_of(deck: &Deck, path: &str) -> String {
    let text = deck.read(path).unwrap().unwrap();
    parse_card(path, &text).id().unwrap().to_owned()
}

#[test]
fn catalog_selects_top_cards_and_folds_by_directory() {
    let home = tempfile::tempdir().unwrap();
    let mut paths: Vec<String> = (0..32).map(|i| format!("d/c{i:02}")).collect();
    paths.push("root".into());
    paths.push("solo".into());
    let (decks, deck) = deck_with(home.path(), &paths);
    let now = rumu_core::now();

    // The 30 `c00`..`c29` cards outrank the rest.
    for i in 0..30 {
        usage_index::bump(&deck.dir, &id_of(&deck, &format!("d/c{i:02}")), 100.0, now);
    }
    // One leftover `d` card beats the root group's total.
    usage_index::bump(&deck.dir, &id_of(&deck, "d/c30"), 1.0, now);

    let note = deck_catalog(&decks, &deck, "/cards/self", deck.id(), now).unwrap();
    assert_eq!(note.scope.deck, deck.id());
    assert_eq!(note.scope.at, "/cards/self");
    assert_eq!(note.entries.len(), CATALOG_BUDGET);
    assert_eq!(
        note.entries.first().map(|e| e.id.as_str()),
        Some("self/d/c00")
    );
    assert_eq!(
        note.entries.last().map(|e| e.id.as_str()),
        Some("self/d/c29")
    );
    assert!(
        note.entries.windows(2).all(|w| w[0].id < w[1].id),
        "entries are in path order"
    );

    // Directories by total score; cards at the root form the `""` group.
    assert_eq!(note.folded.len(), 2);
    assert_eq!(note.folded[0].prefix, "d");
    assert_eq!(note.folded[0].names, ["c30", "c31"]);
    assert_eq!(note.folded[1].prefix, "");
    assert_eq!(note.folded[1].names.len(), 2);
    assert!(note.folded[1].names.iter().any(|n| n == "root"));
    assert!(note.folded[1].names.iter().any(|n| n == "solo"));

    let text = note.markdown();
    assert!(
        text.contains("**Card catalog** (`@rumu:/cards/self`)"),
        "{text}"
    );
    assert!(
        text.contains("[self/d/c00](@rumu:/cards/self/d/c00)"),
        "{text}"
    );
    assert!(text.contains("`d/` (2 more cards: c30, c31)"), "{text}");
    assert!(text.contains("(deck root) (2 more cards: "), "{text}");
    assert!(text.contains("root") && text.contains("solo"), "{text}");
}

#[test]
fn a_small_deck_lists_everything() {
    let home = tempfile::tempdir().unwrap();
    let (decks, deck) = deck_with(home.path(), &["a".into(), "d/b".into()]);
    let note = deck_catalog(&decks, &deck, "/cards/self", deck.id(), rumu_core::now()).unwrap();
    assert_eq!(note.entries.len(), 2);
    assert!(note.folded.is_empty());
}
