//! Library page logic tests: create, edit (CAS), moves, history and search.

use super::*;
use crate::deck::Decks;

/// A home with two decks: `main` (an assistant) and `other`.
fn test_decks() -> (tempfile::TempDir, Decks, Deck, Deck) {
    let dir = tempfile::tempdir().unwrap();
    let decks = Decks::load(dir.path()).unwrap();
    let new = |name: &str, assistant: bool| DeckMeta {
        id: String::new(),
        name: name.into(),
        assistant,
        project: false,
        private: false,
        mounts: Vec::new(),
        allow_ask: Vec::new(),
        mcp: Default::default(),
        icon: None,
    };
    let main = decks.upsert(new("main", true)).unwrap();
    let other = decks.upsert(new("other", false)).unwrap();
    (dir, decks, main, other)
}

#[test]
fn create_lists_keys_and_records_the_user() {
    let (_dir, decks, main, _other) = test_decks();
    let saved = save(
        &decks,
        &main,
        SaveRequest {
            path: "notes/a",
            view: "---\ndescription: d\nkeywords: [k]\npinned: true\n---\nbody\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    assert_eq!(saved.card.path, "notes/a");
    assert!(saved.card.pinned);
    assert_eq!(saved.card.keywords, ["k"]);
    assert_eq!(saved.card.updated_by.as_deref(), Some("user"));

    // A new deck starts with its charter, `about`.
    let rows = cards(&decks, Some(&main)).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].path, "about");
    assert_eq!(rows[1].path, "notes/a");
    assert_eq!(rows[1].updated_by.as_deref(), Some("user"));
}

#[test]
fn edit_uses_the_view_rev_as_cas() {
    let (_dir, decks, main, _other) = test_decks();
    let created = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "one\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    let updated = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "two\n",
            base: &created.view_rev,
            from: None,
        },
    )
    .unwrap();
    assert_ne!(updated.view_rev, created.view_rev);

    // The old base no longer matches.
    let err = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "three\n",
            base: &created.view_rev,
            from: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, Error::Stale { .. }), "{err}");
    assert_eq!(detail(&decks, &main, "a").unwrap().view, "two\n");
}

#[test]
fn a_move_is_a_new_card_in_history() {
    let (_dir, decks, main, _other) = test_decks();
    let created = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "one\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    let updated = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "two\n",
            base: &created.view_rev,
            from: None,
        },
    )
    .unwrap();
    let moved = save(
        &decks,
        &main,
        SaveRequest {
            path: "sub/b",
            view: &updated.view,
            base: &updated.view_rev,
            from: Some((&main, "a")),
        },
    )
    .unwrap();
    assert_eq!(moved.card.path, "sub/b");
    assert!(
        cards(&decks, Some(&main))
            .unwrap()
            .iter()
            .all(|card| card.path != "a")
    );

    let old = history(&decks, &main, "a").unwrap();
    let subjects: Vec<&str> = old.iter().map(|commit| commit.subject.as_str()).collect();
    assert!(subjects.contains(&"create a"), "{subjects:?}");
    assert!(subjects.contains(&"update a"), "{subjects:?}");
    assert!(subjects.contains(&"move a -> sub/b"), "{subjects:?}");
    // The new path only knows the move.
    assert_eq!(history(&decks, &main, "sub/b").unwrap().len(), 1);
}

#[test]
fn a_cross_deck_move_lands_in_the_destination() {
    let (_dir, decks, main, other) = test_decks();
    let created = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "one\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    let moved = save(
        &decks,
        &other,
        SaveRequest {
            path: "a",
            view: &created.view,
            base: &created.view_rev,
            from: Some((&main, "a")),
        },
    )
    .unwrap();
    assert_eq!(moved.card.deck, other.id());
    assert!(
        cards(&decks, Some(&main))
            .unwrap()
            .iter()
            .all(|card| card.path != "a")
    );
    assert!(
        cards(&decks, Some(&other))
            .unwrap()
            .iter()
            .any(|card| card.path == "a")
    );
}

#[test]
fn version_returns_hunks_for_an_update() {
    let (_dir, decks, main, _other) = test_decks();
    let created = save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "one\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "one\ntwo\n",
            base: &created.view_rev,
            from: None,
        },
    )
    .unwrap();
    let commits = history(&decks, &main, "a").unwrap();
    let update = commits
        .iter()
        .find(|commit| commit.subject == "update a")
        .expect("the update commit");
    let version = version(&decks, &main, "a", &update.commit).unwrap();
    assert!(
        version
            .view
            .as_deref()
            .is_some_and(|view| view.contains("two"))
    );
    assert!(version.previous.is_some());
    assert!(!version.hunks.is_empty());
}

#[test]
fn search_finds_a_body_word_and_returns_a_snippet() {
    let (_dir, decks, main, _other) = test_decks();
    save(
        &decks,
        &main,
        SaveRequest {
            path: "a",
            view: "the zebra crossing is quiet\n",
            base: "",
            from: None,
        },
    )
    .unwrap();
    let hits = search(&decks, "zebra", None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "a");
    assert!(
        hits[0]
            .snippet
            .as_deref()
            .is_some_and(|snippet| snippet.contains("zebra"))
    );
}

fn create(decks: &Decks, deck: &Deck, path: &str) -> LibraryCardDetail {
    save(
        decks,
        deck,
        SaveRequest {
            path,
            view: "---\ndescription: d\n---\nbody\n",
            base: "",
            from: None,
        },
    )
    .unwrap()
}

#[test]
fn a_folder_move_moves_every_card() {
    let (_dir, decks, _main, other) = test_decks();
    create(&decks, &other, "notes/a");
    create(&decks, &other, "notes/sub/b");
    create(&decks, &other, "notes-x");

    move_path(&decks, &other, "notes", "archive/notes", true).unwrap();

    let rows = cards(&decks, Some(&other)).unwrap();
    let paths: Vec<_> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        ["about", "archive/notes/a", "archive/notes/sub/b", "notes-x"]
    );
    assert!(
        cards(&decks, Some(&other))
            .unwrap()
            .iter()
            .all(|card| !card.path.starts_with("notes/"))
    );
    assert_eq!(
        history(&decks, &other, "archive/notes/a").unwrap()[0]
            .path
            .as_deref(),
        Some("archive/notes/a")
    );
}

#[test]
fn a_move_refuses_an_existing_destination_and_a_move_into_itself() {
    let (_dir, decks, _main, other) = test_decks();
    create(&decks, &other, "a");
    create(&decks, &other, "b");
    create(&decks, &other, "dir/c");
    assert!(move_path(&decks, &other, "a", "b", false).is_err());
    assert!(move_path(&decks, &other, "dir", "dir/inner", true).is_err());
    move_path(&decks, &other, "a", "renamed", false).unwrap();
    let paths: Vec<_> = cards(&decks, Some(&other))
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["about", "b", "dir/c", "renamed"]);
}

#[test]
fn a_folder_delete_removes_only_its_cards() {
    let (_dir, decks, _main, other) = test_decks();
    create(&decks, &other, "dir/a");
    create(&decks, &other, "dir/deep/b");
    create(&decks, &other, "keep");
    delete_folder(&decks, &other, "dir").unwrap();
    let paths: Vec<_> = cards(&decks, Some(&other))
        .unwrap()
        .into_iter()
        .map(|r| r.path)
        .collect();
    assert_eq!(paths, ["about", "keep"]);
    assert!(
        cards(&decks, Some(&other))
            .unwrap()
            .iter()
            .all(|card| !card.path.starts_with("dir/"))
    );
}

#[test]
fn a_deck_delete_refuses_assistants_and_removes_plain_decks() {
    let (_dir, decks, main, other) = test_decks();
    create(&decks, &other, "a");
    assert!(decks.delete(main.id()).is_err());
    decks.delete(other.id()).unwrap();
    assert!(decks.get(other.id()).is_none());
    assert!(!other.dir.exists());
}
