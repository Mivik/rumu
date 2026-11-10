//! Tests for the memory plugin: the built-in hooks (catalog, recall, update
//! notices) and revision-aware imports.

use super::*;
use crate::testutil::{FakeRuntime, fixture_copy};
use rumu_cards::catalog::{CatalogNote, CatalogScope};
use rumu_core::{Event, Node, NodeBody};
use serde_json::json;

/// The memory plugin over a temp home's decks.
fn memory_plugin(home: &std::path::Path) -> MemoryPlugin {
    MemoryPlugin::new(rumu_cards::Decks::load(home).unwrap())
}

/// One card read from the `main` deck's `HEAD` tree.
fn deck_card(home: &std::path::Path, path: &str) -> rumu_cards::Card {
    let text = crate::testutil::card_text(home, "main", path).unwrap();
    rumu_cards::card::parse_card(path, &text)
}

fn event(kind: &str) -> Event {
    Event {
        kind: kind.into(),
        session: Default::default(),
        branch: Default::default(),
        fields: rumu_core::Fields {
            event: kind.into(),
            ..Default::default()
        },
        payload: serde_json::Value::Null,
    }
}

fn path_note(id: &str, rev: &str, text: &str) -> Node {
    Node {
        id: rumu_core::NodeId::new(),
        parent: None,
        session: Default::default(),
        at: 0,
        dur_ms: None,
        body: NodeBody::Note {
            source: "memory".into(),
            content: rumu_core::Content::note_with(
                text,
                &usage::CardImport {
                    deck: crate::testutil::MAIN_DECK.into(),
                    path: id.into(),
                    rev: rev.into(),
                },
            ),
        },
    }
}

/// A path node recording that a catalog note was already injected.
fn path_note_with_catalog() -> Node {
    let catalog = CatalogNote {
        scope: CatalogScope {
            deck: crate::testutil::MAIN_DECK.into(),
            at: "/cards/self".into(),
        },
        entries: Vec::new(),
        folded: Vec::new(),
    };
    Node {
        id: rumu_core::NodeId::new(),
        parent: None,
        session: Default::default(),
        at: 0,
        dur_ms: None,
        body: NodeBody::Note {
            source: "memory".into(),
            content: rumu_core::Content::note_with("catalog", &catalog),
        },
    }
}

fn path_use(id: &str, rev: &str, call_id: &str) -> Vec<Node> {
    deck_use(crate::testutil::MAIN_DECK, id, rev, call_id)
}

/// [`path_use`] of a card in any deck.
fn deck_use(deck: &str, id: &str, rev: &str, call_id: &str) -> Vec<Node> {
    vec![
        Node {
            id: rumu_core::NodeId::new(),
            parent: None,
            session: Default::default(),
            at: 0,
            dur_ms: None,
            body: NodeBody::ToolResult {
                call_id: call_id.into(),
                content: rumu_core::Content::text("ok"),
                is_error: false,
                invoke: None,
            },
        },
        Node {
            id: rumu_core::NodeId::new(),
            parent: None,
            session: Default::default(),
            at: 0,
            dur_ms: None,
            body: NodeBody::Note {
                source: "workspace".into(),
                content: usage::card_use(
                    deck,
                    id,
                    rev,
                    "v0",
                    None,
                    usage::CardOp::Read,
                    Some(call_id),
                ),
            },
        },
    ]
}

// ---------- manifest ----------

#[tokio::test]
async fn manifest_lists_the_built_in_hooks() {
    let home = fixture_copy();
    let plugin = memory_plugin(home.path());
    let manifest = plugin.manifest();
    assert_eq!(manifest.name, "memory");
    assert!(manifest.instructions.is_some());

    // The memory plugin registers no tools of its own (discovery belongs to
    // the workspace plugin's `search`).
    assert!(manifest.tools.is_empty());

    let hooks: Vec<(&str, i32)> = manifest
        .hooks
        .iter()
        .map(|h| (h.id.as_str(), h.priority))
        .collect();
    // On `llm.pre`: the assistant introduction, then the catalog index, then the
    // update notices last.
    assert_eq!(
        hooks,
        [
            ("assistant-intro", i32::MAX - 2),
            ("card-catalog", i32::MAX - 1),
            ("card-updates", i32::MAX),
            ("recall", 0)
        ]
    );

    // Unknown hook id → no effects.
    let host = FakeRuntime::arc();
    let out = plugin
        .on_hook("nope", &event("tool.pre"), &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty());
}

// ---------- recall ----------

/// A temp home with the given `(id, frontmatter, body)` cards.
fn recall_home(cards: &[(&str, &str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(dir.path());
    for (id, front, body) in cards {
        crate::testutil::put_card(dir.path(), "main", id, format!("---\n{front}\n---\n{body}"));
    }
    dir
}

fn prompt(text: &str) -> Event {
    let mut ev = event("prompt");
    ev.fields.text = Some(text.into());
    ev.payload = json!({ "text": text });
    ev
}

#[tokio::test]
async fn recall_inlines_the_matching_card() {
    let home = recall_home(&[
        (
            "deploy",
            "description: k8s rollout\nkeywords: [deploy, rollout]",
            "the card body\n",
        ),
        ("other", "keywords: [unrelated]", "other body\n"),
    ]);
    let plugin = memory_plugin(home.path());
    let host = FakeRuntime::arc();

    // The keyword match inlines the body; the unrelated card stays out.
    let out = plugin
        .on_hook("recall", &prompt("how do I deploy?"), &host.cx())
        .await
        .unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    assert_eq!(out.effects[0].text, "# self/deploy\nthe card body\n");
    let import: usage::CardImport = out.effects[0].decode().expect("memory.import payload");
    assert_eq!(import.deck, crate::testutil::MAIN_DECK);
    assert_eq!(import.path, "deploy");

    // The same revision is not imported twice.
    host.apply_output(
        "call-1",
        &rumu_core::ToolOutput::ok("ok").with_effects(out.effects.clone()),
    );
    let out = plugin
        .on_hook("recall", &prompt("deploy again"), &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // A card already used on the path is not imported again.
    let host = FakeRuntime::arc();
    let card = deck_card(home.path(), "deploy");
    host.nodes
        .lock()
        .unwrap()
        .push(path_note(&card.path, &card.revision, "# Deploying\nbody"));
    let out = plugin
        .on_hook("recall", &prompt("deploy"), &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);
}

#[tokio::test]
async fn recall_skips_bodies_over_the_budget() {
    let big = "x".repeat(RECALL_BUDGET);
    let home = recall_home(&[("big", "title: Big\nkeywords: [deploy]", &big)]);
    let plugin = memory_plugin(home.path());
    let out = plugin
        .on_hook("recall", &prompt("deploy"), &FakeRuntime::arc().cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);
}

// ---------- update notices ----------

/// A temp home with one plain card `x`.
fn card_home(body: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(dir.path());
    crate::testutil::put_card(dir.path(), "main", "x", body);
    dir
}

#[tokio::test]
async fn update_notices() {
    let home = card_home("v1\n");
    crate::testutil::put_card(home.path(), "main", "gone", "gone body\n");
    let plugin = memory_plugin(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let card = deck_card(home.path(), "x");
    // Same revision → silent.
    host.nodes
        .lock()
        .unwrap()
        .extend(path_use(&card.path, &card.revision, "call-1"));
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // New revision → one-line notice, no body, key carries the new revision.
    crate::testutil::put_card(home.path(), "main", "x", "v2\n");
    let v2 = deck_card(home.path(), "x").revision;
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    {
        let content = &out.effects[0];
        let update: usage::CardUpdate = content.decode().expect("memory.card_update payload");
        assert_eq!(update.deck, crate::testutil::MAIN_DECK);
        assert_eq!(update.path, "x");
        assert_eq!(update.rev, v2);
        assert!(
            content.text.contains(&format!("Card `@rumu:/cards/self/x` changed since this branch last read it (rev {} → {v2}). Re-read it if it is still relevant.", card.revision)),
            "text shows old → new rev: {}",
            content.text
        );
        assert!(
            !content.text.contains("v2\n"),
            "notice must not carry the body: {}",
            content.text
        );
    }

    // Deleted card → @deleted notice.
    crate::testutil::remove_card(home.path(), "main", "gone");
    host.nodes.lock().unwrap().clear();
    host.nodes
        .lock()
        .unwrap()
        .extend(path_use("gone", "a1b2c3d4", "call-9"));
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    {
        let content = &out.effects[0];
        let update: usage::CardUpdate = content.decode().expect("memory.card_update payload");
        assert_eq!(update.path, "gone");
        assert_eq!(update.rev, "deleted");
        assert!(content.text.contains("deleted"), "{}", content.text);
        assert!(
            content.text.contains("@rumu:/cards/self/gone"),
            "{}",
            content.text
        );
    }
}

/// A deck that is not mounted sends no update notices, except for a card a
/// mounted link still shares.
#[tokio::test]
async fn unmounted_decks_send_no_update_notices() {
    const OTHER: &str = "0193a2f1-0000-7000-8000-00000000b2b2";
    let home = card_home("v1\n");
    crate::testutil::create_deck(
        home.path(),
        "other",
        Some(&format!("id: {OTHER}\nname: other\n")),
        &[("t", "t1\n"), ("u", "u1\n")],
    );
    crate::testutil::put_card(
        home.path(),
        "main",
        "l",
        format!("---\nlink: {OTHER}:t\n---\n"),
    );
    let plugin = memory_plugin(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    for path in ["t", "u"] {
        let text = crate::testutil::card_text(home.path(), "other", path).unwrap();
        let card = rumu_cards::card::parse_card(path, &text);
        host.nodes.lock().unwrap().extend(deck_use(
            OTHER,
            path,
            &card.revision,
            &format!("call-{path}"),
        ));
        crate::testutil::put_card(home.path(), "other", path, format!("{path}2\n"));
    }
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    let paths: Vec<String> = out
        .effects
        .iter()
        .map(|e| e.decode::<usage::CardUpdate>().unwrap().path)
        .collect();
    assert_eq!(paths, ["t"], "{:?}", out.effects);
    assert!(
        out.effects[0].text.contains("other:t"),
        "{}",
        out.effects[0].text
    );
}

// ---------- card catalog ----------
/// The `card-catalog` hook injects one note per mounted deck on the first
/// `llm.pre` of a path and stays silent once a catalog is on the path.
#[tokio::test]
async fn catalog_notes_are_per_deck_and_injected_once() {
    let home = card_home("the body summary\n");
    let plugin = memory_plugin(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let ev = event("llm.pre");

    let out = plugin.on_hook("card-catalog", &ev, &cx).await.unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    {
        let content = &out.effects[0];
        let catalog: CatalogNote = content.decode().expect("structured catalog");
        assert_eq!(catalog.scope.deck, crate::testutil::MAIN_DECK);
        assert_eq!(catalog.scope.at, "/cards/self");
        assert!(
            catalog
                .entries
                .iter()
                .any(|e| e.id == "self/x" && e.summary.as_deref() == Some("the body summary")),
            "{:?}",
            catalog.entries
        );
        assert_eq!(content.text, catalog.markdown());
        assert!(
            content
                .text
                .starts_with("**Card catalog** (`@rumu:/cards/self`)"),
            "{}",
            content.text
        );
    }

    // A catalog is already on the path → nothing more.
    host.nodes.lock().unwrap().push(path_note_with_catalog());
    let out = plugin
        .on_hook("card-catalog", &ev, &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // No decks mounted: nothing at all.
    let empty = tempfile::tempdir().unwrap();
    let plugin = memory_plugin(empty.path());
    let host = FakeRuntime::arc();
    host.state.lock().unwrap().mounts.clear();
    let out = plugin
        .on_hook("card-catalog", &ev, &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);
}

#[tokio::test]
async fn catalog_names_the_last_author() {
    let dir = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(dir.path());
    crate::testutil::put_card(dir.path(), "main", "a", "body\n");
    let plugin = memory_plugin(dir.path());
    let host = FakeRuntime::arc();
    let out = plugin
        .on_hook("card-catalog", &event("llm.pre"), &host.cx())
        .await
        .unwrap();
    let catalog: CatalogNote = out.effects[0].decode().unwrap();
    assert_eq!(catalog.entries[0].id, "self/a");
    assert_eq!(
        catalog.entries[0].author.as_deref(),
        Some("user"),
        "the user wrote it"
    );

    // A real edit by another assistant is named.
    let decks = rumu_cards::Decks::load(dir.path()).unwrap();
    let id = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let cards = rumu_cards::Cards::new(
        &decks,
        &[rumu_core::CardMount {
            deck: id,
            at: "/cards/self".into(),
        }],
    );
    let card = deck_card(dir.path(), "a");
    let mut view = rumu_cards::card::view_of(&card);
    view.push_str("changed\n");
    cards
        .write(
            "self/a",
            &view,
            None,
            None,
            &rumu_cards::deck::Author {
                id: "other-id".into(),
                name: "other-assistant".into(),
                session: None,
                node: None,
            },
        )
        .unwrap();
    let out = plugin
        .on_hook("card-catalog", &event("llm.pre"), &host.cx())
        .await
        .unwrap();
    let catalog: CatalogNote = out.effects[0].decode().unwrap();
    assert_eq!(
        catalog.entries[0].author.as_deref(),
        Some("other-assistant")
    );

    // The assistant's own edit is not named, even one committed under a
    // name the deck no longer has: authors are compared by id.
    let card = deck_card(dir.path(), "a");
    let mut view = rumu_cards::card::view_of(&card);
    view.push_str("mine\n");
    cards
        .write(
            "self/a",
            &view,
            None,
            None,
            &rumu_cards::deck::Author {
                id: crate::testutil::TEST_ASSISTANT.into(),
                name: "old-name".into(),
                session: None,
                node: None,
            },
        )
        .unwrap();
    let out = plugin
        .on_hook("card-catalog", &event("llm.pre"), &host.cx())
        .await
        .unwrap();
    let catalog: CatalogNote = out.effects[0].decode().unwrap();
    assert_eq!(catalog.entries[0].author, None);
}

#[tokio::test]
async fn assistant_intro_names_the_project_and_its_workspaces() {
    const PROJECT: &str = "0193a2f1-0000-7000-8000-00000000c3c3";
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "proj",
        Some(&format!("id: {PROJECT}\nname: proj\nproject: true\n")),
        &[],
    );
    let app_dir = home.path().join("app-ws");
    let proj_dir = home.path().join("proj-ws");
    std::fs::create_dir_all(&app_dir).unwrap();
    std::fs::create_dir_all(&proj_dir).unwrap();
    let plugin = memory_plugin(home.path());
    let host = FakeRuntime::arc();
    {
        let mut state = host.state.lock().unwrap();
        state.workspaces = vec![
            rumu_core::Workspace::new(
                "app".into(),
                None,
                app_dir.to_string_lossy().into_owned(),
                true,
            ),
            rumu_core::Workspace::new(
                "proj".into(),
                None,
                proj_dir.to_string_lossy().into_owned(),
                false,
            ),
        ];
        state.mounted = vec!["app".into(), "proj".into()];
        state.project = Some(PROJECT.into());
        state.mounts.push(rumu_core::CardMount {
            deck: PROJECT.into(),
            at: rumu_core::PROJECT_MOUNT.into(),
        });
    }
    let out = plugin
        .on_hook("assistant-intro", &event("llm.pre"), &host.cx())
        .await
        .unwrap();
    let text = &out.effects[0].text;
    assert!(
        text.contains(&format!("Project: `proj` (deck id {PROJECT})")),
        "{text}"
    );
    assert!(text.contains("@rumu:/cards/project"), "{text}");
    let workspaces = text
        .lines()
        .find(|l| l.contains("Its workspaces"))
        .unwrap_or_else(|| panic!("{text}"));
    let list = &workspaces[workspaces.find("Its workspaces").unwrap()..];
    assert!(
        list.find("`app`").unwrap() < list.find("`proj`").unwrap(),
        "default first: {workspaces}"
    );
    assert!(list.contains(", ro)"), "{workspaces}");
    let project_mount = text
        .lines()
        .find(|l| l.contains("@rumu:/cards/project"))
        .unwrap();
    assert!(
        !project_mount.contains("writes ask the user"),
        "the session's project deck is its own: {project_mount}"
    );
    // Rules: project facts go to `/cards/project`, work under way is
    // qualified by "outside any project".
    assert!(text.contains("goes to `/cards/project`"), "{text}");
    assert!(
        text.contains("work under way outside any project"),
        "{text}"
    );

    // No project: the line says so.
    let out = plugin
        .on_hook(
            "assistant-intro",
            &event("llm.pre"),
            &FakeRuntime::arc().cx(),
        )
        .await
        .unwrap();
    assert!(
        out.effects[0]
            .text
            .contains("This session is in no project.")
    );
}
