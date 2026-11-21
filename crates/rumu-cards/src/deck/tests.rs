//! Deck registry and commit tests.

use super::*;
use crate::{Cards, card};

/// Create a deck under `decks/<id>` and write `cards` through the deck APIs.
fn create_deck(home: &std::path::Path, name: &str, cards: &[(&str, &str)]) -> Deck {
    let decks = Decks::load(home).unwrap();
    let deck = decks
        .upsert(DeckMeta {
            id: String::new(),
            name: name.into(),
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
        })
        .unwrap();
    for (path, text) in cards {
        deck.write_file(path, text, &Author::user()).unwrap();
    }
    deck
}

fn deck_home() -> (tempfile::TempDir, Decks, Deck) {
    let dir = tempfile::tempdir().unwrap();
    create_deck(dir.path(), "main", &[("a", "one\n")]);
    let decks = Decks::load(dir.path()).unwrap();
    let deck = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap();
    (dir, decks, deck)
}

fn latest(deck: &Deck) -> git::Commit {
    deck.commits(1).unwrap().remove(0)
}

#[test]
fn commits_bump_usage_and_a_move_keeps_the_score() {
    let (_dir, _decks, deck) = deck_home();
    let text = deck.read("a").unwrap().unwrap();
    let id = card::parse_card("a", &text).id().unwrap().to_owned();
    let score = || {
        crate::usage_index::scores(&deck.dir, rumu_core::now())
            .get(&id)
            .copied()
            .unwrap_or(0.0)
    };
    // `deck_home` wrote `a` in one commit.
    assert!(score() > 0.0);
    let before = score();

    // A move within the deck keeps the identity and bumps it.
    deck.move_file("a", "sub/b", &Author::user()).unwrap();
    assert!(score() > before, "the move keeps the score");

    // Deleting the card drops the entry.
    deck.remove_file("sub/b", &Author::user()).unwrap();
    assert_eq!(score(), 0.0);
}

#[test]
fn malformed_deck_yaml_is_skipped_on_load() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(DECKS_DIR).join("broken");
    let repo = git::repo(&dir);
    repo.init().unwrap();
    repo.commit(
        &Author::user().git(),
        "bad",
        &git::Trailers {
            changes: &BTreeMap::from([(DECK_FILE.to_owned(), Some("id: [\n".to_owned()))]),
            session: None,
            node: None,
        },
    )
    .unwrap();
    let decks = Decks::load(home.path()).unwrap();
    assert!(decks.list().iter().all(|d| d.meta.name != "broken"));
    assert!(
        decks
            .get(&decks.default_assistant())
            .is_some_and(|d| d.assistant())
    );
}

#[test]
fn an_unreadable_grants_file_is_never_replaced() {
    let (dir, decks, deck) = deck_home();
    let path = dir.path().join(GRANTS_FILE);
    let default = decks.default_assistant();
    std::fs::write(&path, "{ not json").unwrap();
    decks.remember(Action::Write, Scope::Always, "s", &default, deck.id());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    assert!(!decks.allowed(Action::Write, "s", &default, deck.id()));
    // A fixed file is picked up without a restart.
    std::fs::write(&path, "{}").unwrap();
    decks.remember(Action::Write, Scope::Always, "s", &default, deck.id());
    assert!(decks.allowed(Action::Write, "s", &default, deck.id()));
    assert!(decks.allowed(Action::Write, "other session", &default, deck.id()));
    assert!(!decks.allowed(Action::Mount, "s", &default, deck.id()));
}

#[test]
fn approvals_last_as_long_as_their_scope() {
    let (_dir, decks, deck) = deck_home();
    let from = decks.default_assistant();
    assert!(decks.allowed(Action::Write, "s", &from, &from));
    assert!(!decks.allowed(Action::Write, "s", &from, deck.id()));
    decks.remember(Action::Write, Scope::Once, "s", &from, deck.id());
    assert!(!decks.allowed(Action::Write, "s", &from, deck.id()));
    decks.remember(Action::Write, Scope::Session, "s", &from, deck.id());
    assert!(decks.allowed(Action::Write, "s", &from, deck.id()));
    assert!(!decks.allowed(Action::Write, "t", &from, deck.id()));
    assert!(!decks.allowed(Action::Link, "s", &from, deck.id()));
}

#[test]
fn deck_yaml_mounts_grant_mounting_and_marked_writes() {
    let (_dir, decks, deck) = deck_home();
    let from = decks.default_assistant();
    let mut meta = decks.get(&from).unwrap().meta;
    meta.mounts = vec![DeckMount {
        deck: deck.id().to_owned(),
        at: "/cards/main".into(),
        write: false,
    }];
    decks.upsert(meta.clone()).unwrap();
    assert!(decks.allowed(Action::Mount, "s", &from, deck.id()));
    assert!(!decks.allowed(Action::Write, "s", &from, deck.id()));
    meta.mounts[0].write = true;
    decks.upsert(meta).unwrap();
    assert!(decks.allowed(Action::Write, "s", &from, deck.id()));
}

#[test]
fn each_write_is_one_commit_with_author_and_trailers() {
    let (_dir, decks, deck) = deck_home();
    let cards = Cards::new(&decks, &[CardMount::new(deck.id(), "/cards/self", false)]);
    let author = Author {
        id: "deck-id".into(),
        name: "ledger".into(),
        session: Some("s1".into()),
        node: Some("n1".into()),
    };
    cards
        .write("self/x", "body\n", None, None, &author)
        .unwrap();
    let commit = latest(
        &decks
            .list()
            .into_iter()
            .find(|d| d.meta.name == "main")
            .unwrap(),
    );
    assert_eq!(commit.subject, "create x");
    assert_eq!(commit.author, "ledger");
    assert_eq!(commit.email, "deck-id@rumu");
    assert_eq!(commit.changed, ["x"]);
    assert_eq!(commit.session.as_deref(), Some("s1"));
    assert_eq!(commit.node.as_deref(), Some("n1"));

    let mut view = card::view_of(&cards.raw("self/x").unwrap().unwrap());
    view.push_str("more\n");
    cards.write("self/x", &view, None, None, &author).unwrap();
    let commit = latest(
        &decks
            .list()
            .into_iter()
            .find(|d| d.meta.name == "main")
            .unwrap(),
    );
    assert_eq!(commit.subject, "update x");
    assert_eq!(commit.changed, ["x"]);
}

#[test]
fn a_moved_card_is_a_new_card_in_history() {
    let (_dir, decks, deck) = deck_home();
    let cards = Cards::new(&decks, &[CardMount::new(deck.id(), "/cards/self", false)]);
    cards
        .write("self/x", "body\n", None, None, &Author::user())
        .unwrap();
    let view = card::view_of(&cards.raw("self/x").unwrap().unwrap());
    cards
        .write("self/sub/y", &view, Some("self/x"), None, &Author::user())
        .unwrap();
    let deck = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap();
    // The old path's history ends at the removal; the new path starts at the
    // move. The identity (and its usage score) carries over.
    let old = deck.touching("x", None).unwrap();
    assert_eq!(old.len(), 2, "{old:?}");
    assert!(old.iter().any(|c| c.subject == "move x -> sub/y"));
    let new = deck.touching("sub/y", None).unwrap();
    assert_eq!(new.len(), 1, "{new:?}");
    assert!(deck.read("x").unwrap().is_none());
    assert!(deck.read("sub/y").unwrap().is_some());
}

#[test]
fn deck_upsert_rename_and_delete_refresh_the_registry() {
    let (dir, decks, _main) = deck_home();
    let deck = decks
        .upsert(DeckMeta {
            id: String::new(),
            name: "ledger".into(),
            assistant: true,
            project: false,
            private: false,
            mounts: Vec::new(),
            allow_ask: Vec::new(),
            model: None,
            mcp: Default::default(),
            skills: Vec::new(),
            icon: None,
            cat: None,
        })
        .unwrap();
    let id = deck.id().to_owned();
    assert!(dir.path().join("decks").join(&id).join("HEAD").is_file());
    assert_eq!(
        decks
            .list()
            .into_iter()
            .find(|d| d.meta.name == "ledger")
            .unwrap()
            .id(),
        id
    );

    let mut meta = decks.get(&id).unwrap().meta.clone();
    meta.name = "ledger2".into();
    let renamed = decks.upsert(meta).unwrap();
    assert_eq!(renamed.meta.name, "ledger2");
    // The directory is named by id and does not move.
    assert!(dir.path().join("decks").join(&id).join("HEAD").is_file());
    assert!(!dir.path().join("decks/ledger").exists());
    assert!(!dir.path().join("decks/ledger2").exists());
    assert_eq!(
        decks.get(&id).unwrap().dir,
        dir.path().join("decks").join(&id)
    );

    decks.remove_assistant(&id).unwrap();
    assert!(!decks.get(&id).unwrap().assistant());
}

#[test]
fn deck_names_are_display_labels_and_unique() {
    for ok in [
        "main",
        "My Project",
        "我的项目",
        "a b",
        &"x".repeat(DECK_NAME_MAX),
    ] {
        assert!(valid_deck_name(ok), "{ok:?}");
    }
    for bad in ["", "   ", "a\nb", &"x".repeat(DECK_NAME_MAX + 1)] {
        assert!(!valid_deck_name(bad), "{bad:?}");
    }

    let (dir, decks, _main) = deck_home();
    // Names are unique regardless of case; trimming happens on the way in.
    let err = decks.upsert(empty_meta(String::new(), "MAIN".into()));
    assert!(err.is_err());
    let deck = decks
        .upsert(empty_meta(String::new(), "  My Project  ".into()))
        .unwrap();
    assert_eq!(deck.meta.name, "My Project");
    assert_eq!(deck.dir, dir.path().join("decks").join(deck.id()));
}

#[test]
fn a_write_commits_only_its_own_path() {
    let (_dir, decks, deck) = deck_home();
    let cards = Cards::new(&decks, &[CardMount::new(deck.id(), "/cards/self", false)]);
    cards
        .write("self/b", "body b\n", None, None, &Author::for_deck(&deck))
        .unwrap();
    let commit = latest(&deck);
    assert_eq!(commit.changed, ["b"], "{commit:?}");
}

/// `main` at `/cards/self` and a second deck `other` at `/cards/other`.
fn two_decks() -> (tempfile::TempDir, Decks, Cards) {
    let dir = tempfile::tempdir().unwrap();
    create_deck(dir.path(), "main", &[("x", "main x\n")]);
    create_deck(dir.path(), "other", &[("x", "other x\n")]);
    let decks = Decks::load(dir.path()).unwrap();
    let id = |name: &str| {
        decks
            .list()
            .into_iter()
            .find(|d| d.meta.name == name)
            .unwrap()
            .id()
            .to_owned()
    };
    let cards = Cards::new(
        &decks,
        &[
            CardMount::new(id("main"), "/cards/self", false),
            CardMount::new(id("other"), "/cards/other", false),
        ],
    );
    (dir, decks, cards)
}

#[test]
fn card_paths_cannot_leave_their_deck() {
    let (_dir, _decks, cards) = two_decks();
    for bad in [
        "self/../other/x",
        "self",
        "self/.git/config",
        "self/a//b",
        "self/.hidden",
    ] {
        assert!(cards.split(bad).is_none(), "{bad}");
        assert!(cards.raw(bad).unwrap().is_none(), "{bad}");
        assert!(
            cards
                .write(bad, "x\n", None, None, &Author::user())
                .is_err(),
            "{bad}"
        );
    }
    // A move whose source escapes is refused before anything is touched.
    let err = cards
        .write(
            "self/stolen",
            "x\n",
            Some("self/../other/x"),
            None,
            &Author::user(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidPath { .. }), "{err}");
    assert!(cards.raw("other/x").unwrap().is_some());
    assert!(cards.raw("self/stolen").unwrap().is_none());
}

#[test]
fn a_stale_base_revision_is_refused_under_the_lock() {
    let (_dir, _decks, cards) = two_decks();
    let seen = card::view_rev_of(&card::view_of(&cards.raw("self/x").unwrap().unwrap()));
    cards
        .write("self/x", "first\n", None, Some(&seen), &Author::user())
        .unwrap();
    let err = cards
        .write("self/x", "second\n", None, Some(&seen), &Author::user())
        .unwrap_err();
    assert!(matches!(err, Error::Stale { .. }), "{err}");
    assert_eq!(cards.raw("self/x").unwrap().unwrap().body, "first\n");
}

#[test]
fn a_move_checks_its_source_base_under_the_lock() {
    let (_dir, _decks, cards) = two_decks();
    let view_rev =
        |public: &str| card::view_rev_of(&card::view_of(&cards.raw(public).unwrap().unwrap()));
    for (to, from) in [("self/y", "self/x"), ("self/z", "other/x")] {
        let seen = view_rev(from);
        // Another writer changes the source after it was read.
        cards
            .write(from, "changed\n", None, Some(&seen), &Author::user())
            .unwrap();
        let err = cards
            .write(to, "moved\n", Some(from), Some(&seen), &Author::user())
            .unwrap_err();
        assert!(matches!(&err, Error::Stale { id } if id == from), "{err}");
        assert_eq!(cards.raw(from).unwrap().unwrap().body, "changed\n");
        assert!(cards.raw(to).unwrap().is_none());

        let current = view_rev(from);
        cards
            .write(to, "moved\n", Some(from), Some(&current), &Author::user())
            .unwrap();
        assert!(cards.raw(from).unwrap().is_none());
    }
}

#[test]
fn cross_deck_moves_commit_each_side() {
    let (_dir, decks, cards) = two_decks();
    cards
        .write("self/y", "moved\n", Some("other/x"), None, &Author::user())
        .unwrap();
    assert!(cards.raw("self/y").unwrap().is_some());
    assert!(cards.raw("other/x").unwrap().is_none());
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap();
    let other = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "other")
        .unwrap();
    assert_eq!(latest(&main).subject, "move x -> y");
    assert_eq!(latest(&other).subject, "delete x");
}

#[test]
fn links_stay_one_level_deep() {
    let (_dir, _decks, cards) = two_decks();
    cards
        .write(
            "self/l",
            "---\nlink: @rumu:/cards/other/x\n---\n",
            None,
            None,
            &Author::user(),
        )
        .unwrap();
    let view = card::view_of(&cards.raw("other/x").unwrap().unwrap());
    let rev = card::view_rev_of(&view);
    let err = cards
        .write(
            "other/x",
            "---\nlink: @rumu:/cards/self/x\n---\n",
            None,
            Some(&rev),
            &Author::user(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::LinkedTarget { .. }), "{err}");
    let err = cards
        .write(
            "self/l2",
            "---\nlink: @rumu:/cards/self/l\n---\n",
            None,
            None,
            &Author::user(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::LinkToLink { .. }), "{err}");
}

#[test]
fn a_link_holds_only_its_target() {
    let (_dir, _decks, cards) = two_decks();
    cards
        .write(
            "self/l",
            "---\ndescription: own\nkeywords: [own]\npinned: true\nlink: @rumu:/cards/other/x\n---\nown body\n",
            None,
            None,
            &Author::user(),
        )
        .unwrap();
    let raw = cards.raw("self/l").unwrap().unwrap();
    assert_eq!(raw.description, None);
    assert_eq!(raw.body, "");
    let mut keys: Vec<&str> = raw.meta.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["id", "link"]);
}

#[test]
fn remove_deletes_and_commits_as_user() {
    let (_dir, decks, deck) = deck_home();
    let cards = Cards::library(&decks);
    let public = format!("{}/a", deck.id());
    let view = card::view_of(&cards.raw(&public).unwrap().unwrap());
    let rev = card::view_rev_of(&view);
    cards.remove(&public, Some(&rev), &Author::user()).unwrap();
    assert!(deck.read("a").unwrap().is_none());
    let commit = latest(
        &decks
            .list()
            .into_iter()
            .find(|d| d.meta.name == "main")
            .unwrap(),
    );
    assert_eq!(commit.subject, "delete a");
    assert_eq!(commit.author, USER);
    assert_eq!(commit.changed, ["a"]);
}

#[test]
fn remove_refuses_a_stale_base() {
    let (_dir, decks, deck) = deck_home();
    let cards = Cards::library(&decks);
    let public = format!("{}/a", deck.id());
    let err = cards
        .remove(&public, Some("not-the-current-view"), &Author::user())
        .unwrap_err();
    assert!(matches!(err, Error::Stale { .. }), "{err}");
    assert!(deck.read("a").unwrap().is_some());
}

#[test]
fn the_default_assistant_is_a_pointer_that_follows_the_roster() {
    let (dir, decks, main) = deck_home();
    let first = decks.default_assistant();
    assert_eq!(
        decks.get(&first).unwrap().meta.name,
        "default",
        "created when no assistant exists"
    );
    let mut meta = decks.get(&first).unwrap().meta.clone();
    meta.name = "helper".into();
    decks.upsert(meta).unwrap();
    let decks = Decks::load(dir.path()).unwrap();
    assert_eq!(decks.default_assistant(), first);
    assert_eq!(decks.assistants().len(), 1, "no second default is created");

    let other = decks
        .upsert(DeckMeta {
            assistant: true,
            ..empty_meta(String::new(), "other".into())
        })
        .unwrap();
    assert!(
        decks.set_default_assistant(main.id()).is_err(),
        "not an assistant deck"
    );
    decks.set_default_assistant(other.id()).unwrap();
    assert_eq!(
        Decks::load(dir.path()).unwrap().default_assistant(),
        other.id()
    );

    decks.remove_assistant(other.id()).unwrap();
    assert_eq!(decks.default_assistant(), first);

    // Removing the last assistant creates a new default right away, as a
    // restart would, so sessions always have one to run as.
    decks.remove_assistant(&first).unwrap();
    let next = decks.default_assistant();
    assert_ne!(next, first);
    assert!(decks.get(&next).unwrap().assistant());
    assert_eq!(Decks::load(dir.path()).unwrap().default_assistant(), next);

    let err = decks.upsert(DeckMeta {
        id: String::new(),
        ..empty_meta(String::new(), "main".into())
    });
    assert!(err.is_err());
}

#[test]
fn a_fresh_home_gets_user_and_world_mounted_by_the_default_assistant() {
    let dir = tempfile::tempdir().unwrap();
    let decks = Decks::load(dir.path()).unwrap();
    let default = decks.get(&decks.default_assistant()).unwrap();
    let mounted: Vec<(String, String)> = default
        .meta
        .mounts
        .iter()
        .map(|m| (decks.get(&m.deck).unwrap().meta.name.clone(), m.at.clone()))
        .collect();
    assert_eq!(
        mounted,
        [
            ("user".to_owned(), "/cards/user".to_owned()),
            ("world".to_owned(), "/cards/world".to_owned()),
        ]
    );
    for deck in decks.list() {
        let about = deck.pinned().unwrap();
        assert_eq!(about.len(), 1, "{} has its charter pinned", deck.meta.name);
        assert_eq!(about[0].path, ABOUT);
        assert!(about[0].id().is_some());
        assert_eq!(deck.summary(), about[0].description);
    }
}

#[test]
fn a_home_with_decks_but_no_assistant_gets_only_the_assistant() {
    let dir = tempfile::tempdir().unwrap();
    let decks = Decks::load(dir.path()).unwrap();
    let first = decks.default_assistant();
    decks.remove_assistant(&first).unwrap();
    let decks = Decks::load(dir.path()).unwrap();
    let names: BTreeSet<String> = decks.list().into_iter().map(|d| d.meta.name).collect();
    assert_eq!(
        names,
        BTreeSet::from(["default", "default-1", "user", "world"].map(String::from))
    );
    let default = decks.get(&decks.default_assistant()).unwrap();
    assert_eq!(default.meta.name, "default-1");
    assert!(default.meta.mounts.is_empty());
}

#[test]
fn a_project_is_never_an_assistant_and_a_session_belongs_to_one() {
    let (_dir, decks, main) = deck_home();
    let both = decks.upsert(DeckMeta {
        assistant: true,
        project: true,
        ..empty_meta(String::new(), "both".into())
    });
    assert!(
        both.unwrap_err()
            .to_string()
            .contains("both an assistant and a project")
    );

    let project = decks
        .upsert(DeckMeta {
            project: true,
            ..empty_meta(String::new(), "My App".into())
        })
        .unwrap();
    assert!(project.project());
    assert_eq!(decks.projects().len(), 1);
    let other = decks
        .upsert(DeckMeta {
            project: true,
            ..empty_meta(String::new(), "other".into())
        })
        .unwrap();
    let id = |d: &Deck| d.meta.id.clone();

    // A project chosen goes at its name, or where asked when that is free.
    let mounts = decks.session_mounts(main.id(), Some(project.id()), None);
    assert_eq!(
        mounts,
        [CardMount::new(id(&project), "/cards/my-app", true)]
    );
    let mounts = decks.session_mounts(main.id(), Some(project.id()), Some("/cards/x"));
    assert_eq!(mounts, [CardMount::new(id(&project), "/cards/x", true)]);
    assert!(decks.session_mounts(main.id(), None, None).is_empty());

    // A project deck the assistant mounts by default is its sessions'
    // project, unless another one is chosen; the chosen one moves aside.
    let mut meta = main.meta.clone();
    meta.mounts = vec![DeckMount {
        deck: id(&other),
        at: "/cards/my-app".into(),
        write: false,
    }];
    decks.upsert(meta).unwrap();
    let mounts = decks.session_mounts(main.id(), None, None);
    assert_eq!(mounts, [CardMount::new(id(&other), "/cards/my-app", true)]);
    let mounts = decks.session_mounts(main.id(), Some(project.id()), Some("/cards/my-app"));
    assert_eq!(
        mounts,
        [
            CardMount::new(id(&other), "/cards/my-app", false),
            CardMount::new(id(&project), "/cards/my-app-2", true),
        ]
    );
    let mounts = decks.session_mounts(main.id(), Some(other.id()), None);
    assert_eq!(mounts, [CardMount::new(id(&other), "/cards/my-app", true)]);
}

#[test]
fn a_project_icon_drops_blank_parts_and_rejects_bad_ones() {
    let (_dir, decks, _main) = deck_home();
    let icon = |glyph: &str, tone: &str| {
        Some(DeckIcon {
            glyph: Some(glyph.into()),
            tone: Some(tone.into()),
        })
    };
    let project = decks
        .upsert(DeckMeta {
            project: true,
            icon: icon(" 🦊 ", ""),
            ..empty_meta(String::new(), "app".into())
        })
        .unwrap();
    assert_eq!(
        project.meta.icon,
        Some(DeckIcon {
            glyph: Some("🦊".into()),
            tone: None
        })
    );
    let reloaded = Decks::load(&decks.home).unwrap();
    assert_eq!(
        reloaded.get(project.id()).unwrap().meta.icon,
        project.meta.icon
    );

    let mut meta = project.meta.clone();
    meta.icon = icon(" ", "");
    assert_eq!(decks.upsert(meta).unwrap().meta.icon, None);

    for (glyph, tone, err) in [("ab cd", "pine", "glyph"), ("A", "teal", "tone")] {
        let mut meta = project.meta.clone();
        meta.icon = icon(glyph, tone);
        let got = decks.upsert(meta).unwrap_err().to_string();
        assert!(got.contains(err), "{got}");
    }
}

#[test]
fn an_assistant_cat_roundtrips_and_rejects_bad_ones() {
    let (_dir, decks, _main) = deck_home();
    let cat = DeckCat {
        items: vec!["bow".into(), "blush".into()],
        tone: Some("pine".into()),
        wide: -1,
        ears: 2,
    };
    let deck = decks
        .upsert(DeckMeta {
            assistant: true,
            cat: Some(cat.clone()),
            ..empty_meta(String::new(), "kit".into())
        })
        .unwrap();
    let reloaded = Decks::load(&decks.home).unwrap();
    assert_eq!(reloaded.get(deck.id()).unwrap().meta.cat, Some(cat.clone()));

    for (bad, err) in [
        (
            DeckCat {
                tone: Some("teal".into()),
                ..cat.clone()
            },
            "tone",
        ),
        (
            DeckCat {
                wide: 3,
                ..cat.clone()
            },
            "wide",
        ),
        (
            DeckCat {
                items: vec!["Bow!".into()],
                ..cat.clone()
            },
            "items",
        ),
    ] {
        let mut meta = deck.meta.clone();
        meta.cat = Some(bad);
        let got = decks.upsert(meta).unwrap_err().to_string();
        assert!(got.contains(err), "{got}");
    }
}

#[test]
fn a_new_deck_has_a_blank_charter_that_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let deck = create_deck(dir.path(), "notes", &[]);
    assert_eq!(deck.walk().unwrap(), [ABOUT]);
    assert_eq!(deck.summary(), None);
    assert!(deck.pinned().unwrap().is_empty());

    deck.write_file(
        ABOUT,
        "---\npinned: true\ndescription: Reading notes.\n---\nBooks and papers.\n",
        &Author::user(),
    )
    .unwrap();
    assert_eq!(deck.summary().as_deref(), Some("Reading notes."));
    assert_eq!(deck.pinned().unwrap().len(), 1);
}

#[test]
fn private_roundtrips_through_deck_yaml() {
    let yaml = "id: 0193a2f1-0000-7000-8000-00000000b1b1\nname: hidden\nprivate: true\n";
    let meta: DeckMeta = crate::yaml::from_str(yaml).unwrap();
    assert!(meta.private);
    let text = serde_saphyr::to_string(&meta).unwrap();
    assert!(text.contains("private: true"), "{text}");
    let again: DeckMeta = crate::yaml::from_str(&text).unwrap();
    assert_eq!(again, meta);
}

/// The objects a commit writes are ordinary git objects: the git binary reads
/// the deck back, checks it strictly, and sees the same tree and history.
#[test]
fn git_reads_the_objects_a_commit_writes() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let deck = create_deck(
        dir.path(),
        "main",
        &[("a", "one\n"), ("notes/nested", "two\n")],
    );
    deck.write_file("notes/nested", "two!\n", &Author::user())
        .unwrap();
    deck.move_file("a", "b", &Author::user()).unwrap();
    deck.remove_file("b", &Author::user()).unwrap();

    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg(format!("--git-dir={}", deck.dir.display()))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };

    git(&["fsck", "--strict"]);

    let mut theirs: Vec<String> = git(&["ls-tree", "-r", "--name-only", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect();
    theirs.sort();
    let ours: Vec<String> = deck.repo().files().unwrap().into_keys().collect();
    assert_eq!(theirs, ours);

    let subjects: Vec<String> = git(&["log", "--format=%s"])
        .lines()
        .map(str::to_owned)
        .collect();
    let mine: Vec<String> = deck
        .commits(usize::MAX)
        .unwrap()
        .into_iter()
        .map(|commit| commit.subject)
        .collect();
    assert_eq!(subjects, mine);

    let card = deck.read("notes/nested").unwrap().unwrap();
    assert!(card.ends_with("two!\n"), "{card}");
}
