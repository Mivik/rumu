//! Tests for the memory plugin: the built-in hooks (catalog, scope cards,
//! update notices) and revision-aware imports.

use super::*;
use crate::testutil::{FakeRuntime, fixture_home};
use rumu_cards::catalog::{CatalogNote, CatalogScope};
use rumu_cards::store::FsCardStore;
use rumu_core::{Event, Node, NodeBody};
use serde_json::json;

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
                    id: id.into(),
                    rev: rev.into(),
                },
            ),
        },
    }
}

/// A path node recording that the root catalog was already injected.
fn path_note_with_catalog() -> Node {
    let catalog = CatalogNote {
        scope: CatalogScope::Root,
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
                content: usage::card_use(id, rev, "v0", usage::CardOp::Read, Some(call_id)),
            },
        },
    ]
}

// ---------- manifest ----------

#[tokio::test]
async fn manifest_lists_the_built_in_hooks() {
    let plugin = MemoryPlugin::new(&fixture_home());
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
    // The catalog index runs just before the update notices, which run last
    // on `llm.pre`.
    assert_eq!(
        hooks,
        [
            ("card-catalog", i32::MAX - 1),
            ("card-updates", i32::MAX),
            ("related-cards", 0),
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
    std::fs::create_dir_all(dir.path().join("cards")).unwrap();
    for (id, front, body) in cards {
        std::fs::write(
            dir.path().join("cards").join(format!("{id}.md")),
            format!("---\n{front}\n---\n{body}"),
        )
        .unwrap();
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
    let plugin = MemoryPlugin::new(home.path());
    let host = FakeRuntime::arc();

    // The keyword match inlines the body; the unrelated card stays out.
    let out = plugin
        .on_hook("recall", &prompt("how do I deploy?"), &host.cx())
        .await
        .unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    assert_eq!(out.effects[0].text, "# deploy\nthe card body\n");
    let import: usage::CardImport = out.effects[0].decode().expect("memory.import payload");
    assert_eq!(import.id, "deploy");

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
    let card = library::load_card(&FsCardStore::home(home.path()), "deploy")
        .unwrap()
        .unwrap();
    host.nodes
        .lock()
        .unwrap()
        .push(path_note("deploy", &card.revision, "# Deploying\nbody"));
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
    let plugin = MemoryPlugin::new(home.path());
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
    std::fs::create_dir_all(dir.path().join("cards")).unwrap();
    std::fs::write(dir.path().join("cards/x.md"), body).unwrap();
    dir
}

#[tokio::test]
async fn update_notices() {
    let home = card_home("v1\n");
    let plugin = MemoryPlugin::new(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let card = library::load_card(&FsCardStore::home(home.path()), "x")
        .unwrap()
        .unwrap();
    // Same revision → silent.
    host.nodes
        .lock()
        .unwrap()
        .extend(path_use("x", &card.revision, "call-1"));
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // New revision → one-line notice, no body, key carries the new revision.
    std::fs::write(home.path().join("cards/x.md"), "v2\n").unwrap();
    let v2 = library::load_card(&FsCardStore::home(home.path()), "x")
        .unwrap()
        .unwrap()
        .revision;
    let out = plugin
        .on_hook("card-updates", &event("llm.pre"), &cx)
        .await
        .unwrap();
    {
        let content = &out.effects[0];
        let update: usage::CardUpdate = content.decode().expect("memory.card_update payload");
        assert_eq!(update.id, "x");
        assert_eq!(update.rev, v2);
        assert!(
            content.text.contains(&format!("Card `@rumu:/cards/x` changed since this branch last read it (rev {} → {v2}). Re-read it if it is still relevant.", card.revision)),
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
        assert_eq!(update.id, "gone");
        assert_eq!(update.rev, "deleted");
        assert!(content.text.contains("deleted"), "{}", content.text);
        assert!(
            content.text.contains("@rumu:/cards/gone"),
            "{}",
            content.text
        );
    }
}

// ---------- card catalog ----------
/// The `card-catalog` hook injects one root index on the first `llm.pre` of a
/// branch and stays silent once its key is on the path.
#[tokio::test]
async fn catalog_root_note_injected_once() {
    let home = card_home("the body summary\n");
    let plugin = MemoryPlugin::new(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let ev = event("llm.pre");

    let out = plugin.on_hook("card-catalog", &ev, &cx).await.unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    {
        let content = &out.effects[0];
        let catalog: CatalogNote = content.decode().expect("structured catalog");
        assert_eq!(catalog.scope, CatalogScope::Root);
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].id, "x");
        assert_eq!(
            catalog.entries[0].summary.as_deref(),
            Some("the body summary")
        );
        assert_eq!(content.text, catalog.markdown());
        assert!(
            content.text.starts_with("**Card catalog**"),
            "{}",
            content.text
        );
        assert!(
            content.text.contains("[x](@rumu:/cards/x)"),
            "{}",
            content.text
        );
    }

    // The root catalog is already on the path → nothing more.
    host.nodes.lock().unwrap().push(path_note_with_catalog());
    let cx = host.cx();
    let out = plugin.on_hook("card-catalog", &ev, &cx).await.unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // An empty library injects nothing at all.
    let empty = tempfile::tempdir().unwrap();
    let plugin = MemoryPlugin::new(empty.path());
    let host = FakeRuntime::arc();
    let out = plugin
        .on_hook("card-catalog", &ev, &host.cx())
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);
}

// ---------- scope cards (related-cards) ----------

/// A home with the given `(id, scope, body)` cards, the plugin, and a host
/// whose path state has a workspace `proj` rooted at `<home>/proj`.
async fn scope_setup(
    cards: &[(&str, &str, &str)],
) -> (tempfile::TempDir, MemoryPlugin, std::sync::Arc<FakeRuntime>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cards")).unwrap();
    for (id, scope, body) in cards {
        std::fs::write(
            dir.path().join("cards").join(format!("{id}.md")),
            format!("---\ntitle: {id}\nscope: {scope}\n---\n{body}"),
        )
        .unwrap();
    }
    let plugin = MemoryPlugin::new(dir.path());
    let host = FakeRuntime::arc();
    host.state.lock().unwrap().workspaces = vec![rumu_core::Workspace {
        name: "proj".into(),
        ssh: None,
        root: under(dir.path(), "").trim_end_matches('/').into(),
        read_only: false,
    }];
    (dir, plugin, host)
}

/// A successful `tool.post` event as the kernel's field extraction would
/// produce it.
fn tool_post(tool: &str, workspace: &str, paths: Vec<String>) -> Event {
    let mut ev = event("tool.post");
    ev.fields.tool = Some(tool.into());
    if tool == "workspace_use" {
        // `workspace_use` carries its target in args, not fields.
        ev.payload = json!({ "args": { "name": workspace }, "is_error": false });
    } else {
        ev.payload = json!({ "args": {}, "is_error": false });
        ev.fields.workspace = Some(workspace.into());
    }
    ev.fields.paths = paths;
    ev
}

fn under(home: &std::path::Path, rel: &str) -> String {
    home.join("proj").join(rel).to_string_lossy().into_owned()
}

async fn related(
    plugin: &MemoryPlugin,
    host: &std::sync::Arc<FakeRuntime>,
    ev: &Event,
) -> Vec<String> {
    let out = plugin
        .on_hook("related-cards", ev, &host.cx())
        .await
        .unwrap();
    // The kernel's safe point: later calls see these notes on the path.
    host.apply_output(
        "call-1",
        &rumu_core::ToolOutput::ok("ok").with_effects(out.effects.clone()),
    );
    out.effects
        .iter()
        .map(|e| {
            let import: usage::CardImport = e.decode().expect("memory.import payload");
            format!("{}: {}", import.id, e.text)
        })
        .collect()
}

#[tokio::test]
async fn related_cards_follow_workspace_and_path_touches() {
    let (home, plugin, host) = scope_setup(&[
        ("whole", "proj", "workspace notes\n"),
        ("tree", "[proj:crates/rumu-kernel/**]", "tree notes\n"),
        (
            "file",
            "[proj:crates/rumu-kernel/src/dispatch.rs]",
            "file notes\n",
        ),
        ("ghost", "[ghost:crates/**]", "ghost notes\n"),
    ])
    .await;

    // A workspace touch (bash) imports the whole-workspace card only.
    let ev = tool_post("bash", "proj", vec![]);
    assert_eq!(
        related(&plugin, &host, &ev).await,
        ["whole: # whole\nworkspace notes\n"]
    );

    // A file touch imports the rest, most specific first; `whole` is
    // already on the path at this revision, `ghost` names no such workspace.
    let ev = tool_post(
        "read",
        "proj",
        vec![under(home.path(), "crates/rumu-kernel/src/dispatch.rs")],
    );
    assert_eq!(
        related(&plugin, &host, &ev).await,
        ["file: # file\nfile notes\n", "tree: # tree\ntree notes\n"]
    );
    // Touching it again is silent.
    assert!(related(&plugin, &host, &ev).await.is_empty());
}

#[tokio::test]
async fn related_cards_on_workspace_use() {
    let (_home, plugin, host) = scope_setup(&[("whole", "[proj]", "notes\n")]).await;
    let ev = tool_post("workspace_use", "proj", vec![]);
    assert_eq!(
        related(&plugin, &host, &ev).await,
        ["whole: # whole\nnotes\n"]
    );
}

#[tokio::test]
async fn related_cards_reinject_when_the_revision_changed() {
    let (home, plugin, host) = scope_setup(&[("kernel", "[proj:crates/**]", "new body\n")]).await;
    host.nodes
        .lock()
        .unwrap()
        .push(path_note("kernel", "00000000", "# kernel\nold body"));
    let ev = tool_post("read", "proj", vec![under(home.path(), "crates/a.rs")]);
    assert_eq!(
        related(&plugin, &host, &ev).await,
        [
            "kernel: (Updated card — supersedes rev 00000000 read earlier on this branch.)\n\
          # kernel\nnew body\n"
        ]
    );
}

#[tokio::test]
async fn related_cards_ignore_failed_calls() {
    let (home, plugin, host) = scope_setup(&[("kernel", "[proj:crates/**]", "notes\n")]).await;
    let mut ev = tool_post("read", "proj", vec![under(home.path(), "crates/a.rs")]);
    ev.payload["is_error"] = json!(true);
    assert!(related(&plugin, &host, &ev).await.is_empty());
}
