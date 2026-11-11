//! End-to-end card scenarios over a real kernel + the memory & workspace
//! plugins (scripted LLM): only branches that actually read a card get
//! update notices; old snapshots survive card edits (history is never
//! rewritten); forking before a read drops the notice for the new branch.

use std::sync::Arc;

use rumu_core::{Node, NodeBody, Plugin, ToolCall, Workspaces};
use rumu_kernel::{Kernel, LlmResponse, ScriptedLlm};
use rumu_plugins::{MemoryPlugin, WorkspacePlugin};
use rumu_store::Store;
use serde_json::json;

// ---------- scripted LLM (same shape as the kernel test suite) ----------

fn resp_call(call_id: &str, name: &str, args: serde_json::Value) -> LlmResponse {
    LlmResponse::calls([ToolCall::new(call_id, name, args)])
}

// ---------- helpers ----------

fn ok_tool_results(nodes: &[Node]) -> Vec<(String, String)> {
    nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::ToolResult {
                call_id,
                content,
                is_error: false,
                ..
            } => Some((call_id.clone(), content.text.clone())),
            _ => None,
        })
        .collect()
}

fn update_notices(nodes: &[Node]) -> Vec<rumu_cards::usage::CardUpdate> {
    nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::Note { content, .. } => content.decode::<rumu_cards::usage::CardUpdate>(),
            _ => None,
        })
        .collect()
}

fn has_card_use(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        matches!(&n.body,
        NodeBody::Note { content, .. }
            if content
                .decode::<rumu_cards::usage::CardUse>()
                .is_some_and(|u| !u.deck.is_empty() && !u.path.is_empty()))
    })
}

/// A temp RUMU_HOME with one card `x` and no rules. Branch A's use comes
/// purely from a card read, so branch B (which never reads) has no use record.
fn card_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let decks = rumu_cards::Decks::load(&home).unwrap();
    let deck = decks
        .upsert(rumu_cards::DeckMeta {
            id: String::new(),
            name: "main".into(),
            assistant: false,
            project: false,
            private: false,
            mounts: Vec::new(),
            allow_ask: Vec::new(),
            icon: None,
        })
        .unwrap();
    deck.write_file(
        "x",
        "---\ntitle: X\n---\nv1 body\n",
        &rumu_cards::Author::user(),
    )
    .unwrap();
    (dir, home)
}

async fn run_turn(kernel: &Arc<Kernel>, branch: &rumu_core::BranchId, text: &str) {
    let mut rx = kernel.tap_pushes();
    kernel.prompt(branch, text).await.unwrap();
    while let Some(v) = rx.recv().await {
        if v["method"].as_str() == Some("turn.done")
            && v["params"]["branch"].as_str() == Some(branch.0.as_str())
        {
            return;
        }
    }
    panic!("turn never finished on {branch}");
}

// ---------- the scenarios ----------

#[tokio::test]
async fn update_notices_only_follow_actual_reads() {
    let (_dir, home) = card_home();
    // One kernel, two sessions, one script in call order:
    //   A turn 1: read(@rumu:/cards/self/x) + reply;  B turn 1: reply;  A turn 2: reply;
    //   fork turn: reply.
    let decks = rumu_cards::Decks::load(&home).unwrap();
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(MemoryPlugin::new(decks.clone())),
        Arc::new(WorkspacePlugin::new(&home, decks, Workspaces::default())),
    ];
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(ScriptedLlm::strict([
            resp_call("a1", "read", json!({ "path": "@rumu:/cards/self/x" })),
            LlmResponse::text("done-a1"),
            LlmResponse::text("done-b1"),
            LlmResponse::text("done-a2"),
            LlmResponse::text("done-fork"),
        ])),
        plugins,
        Workspaces::default(),
    )
    .unwrap();

    let (a, _) = kernel.create_session(main.clone()).await.unwrap();
    let ba = a.active_branch.clone();
    run_turn(&kernel, &ba, "read card x").await;
    let (nodes_a, _) = kernel.branch_path(&ba).unwrap();

    // The read returned the v1 snapshot and is recorded on the path.
    let results = ok_tool_results(&nodes_a);
    assert_eq!(results.len(), 1);
    assert!(results[0].1.contains("v1 body"));
    assert!(has_card_use(&nodes_a), "card_use marker on A's path");

    // The card changes on disk.
    rumu_cards::open_deck(&home, "main")
        .unwrap()
        .unwrap()
        .write_file(
            "x",
            "---\ntitle: X\n---\nv2 body\n",
            &rumu_cards::Author::user(),
        )
        .unwrap();

    // B (fresh session, same home) never read the card → no notice.
    let (b, _) = kernel.create_session(main.clone()).await.unwrap();
    let bb = b.active_branch.clone();
    run_turn(&kernel, &bb, "hello without reading anything").await;
    let (nodes_b, _) = kernel.branch_path(&bb).unwrap();
    assert!(
        update_notices(&nodes_b).is_empty(),
        "B never read the card: {:?}",
        update_notices(&nodes_b)
    );

    // A, prompted again, gets exactly one update notice (no body inside).
    run_turn(&kernel, &ba, "again").await;
    let (nodes_a2, _) = kernel.branch_path(&ba).unwrap();
    let updates = update_notices(&nodes_a2);
    assert_eq!(
        updates.iter().filter(|u| u.rev != "deleted").count(),
        1,
        "exactly one notice: {updates:?}"
    );
    let notice_text = nodes_a2
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Note { content, .. }
                if content
                    .decode::<rumu_cards::usage::CardUpdate>()
                    .is_some_and(|u| u.rev != "deleted") =>
            {
                Some(content.text.clone())
            }
            _ => None,
        })
        .unwrap();
    assert!(
        notice_text.contains("`@rumu:/cards/self/x`"),
        "{notice_text}"
    );
    assert!(notice_text.contains("Re-read it"), "{notice_text}");
    assert!(
        !notice_text.contains("v2 body"),
        "notice carries no body: {notice_text}"
    );

    // The old snapshot in A's history is unchanged.
    assert!(
        ok_tool_results(&nodes_a2)[0].1.contains("v1 body"),
        "history keeps the v1 snapshot"
    );

    // Fork before the read → no use record → no notice.
    let before_read = nodes_a
        .iter()
        .find(|n| matches!(&n.body, NodeBody::User { .. }))
        .unwrap()
        .id
        .clone();
    let forked = kernel.fork(&before_read).await.unwrap();
    run_turn(&kernel, &forked.id, "fresh start").await;
    let (nodes_f, _) = kernel.branch_path(&forked.id).unwrap();
    assert!(
        !has_card_use(&nodes_f),
        "fork before the read carries no use record"
    );
    assert!(
        update_notices(&nodes_f).is_empty(),
        "fork before the read → no notice: {:?}",
        update_notices(&nodes_f)
    );
}

#[tokio::test]
async fn script_edits_count_as_reads() {
    let (_dir, home) = card_home();
    let decks = rumu_cards::Decks::load(&home).unwrap();
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let memory: Arc<dyn Plugin> = Arc::new(MemoryPlugin::new(decks.clone()));
    let workspace: Arc<dyn Plugin> =
        Arc::new(WorkspacePlugin::new(&home, decks, Workspaces::default()));
    let code = Arc::new(rumu_plugins::CodePlugin::new(&workspace.manifest().tools));
    let edit = |old: &str, new: &str| json!({ "path": "@rumu:/cards/self/x", "old_string": old, "new_string": new });
    // read; a script with two edits in a row, then one that throws after an
    // edit; a direct edit; a reply; a second turn.
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(ScriptedLlm::strict([
            resp_call("r", "read", json!({ "path": "@rumu:/cards/self/x" })),
            resp_call(
                "c1",
                "code",
                json!({ "code": format!(
                    "await rumu.edit({}); await rumu.edit({}); return 'ok';",
                    edit("v1", "v2"),
                    edit("body", "text"),
                ) }),
            ),
            resp_call(
                "c2",
                "code",
                json!({ "code": format!(
                    "await rumu.edit({}); throw new Error('later');",
                    edit("v2", "v3"),
                ) }),
            ),
            resp_call("e", "edit", edit("v3", "v4")),
            LlmResponse::text("done-1"),
            LlmResponse::text("done-2"),
        ])),
        vec![memory, workspace, code],
        Workspaces::default(),
    )
    .unwrap();
    let (s, _) = kernel.create_session(main).await.unwrap();
    let b = s.active_branch.clone();
    run_turn(&kernel, &b, "edit card x").await;
    run_turn(&kernel, &b, "again").await;

    let text = rumu_cards::open_deck(&home, "main")
        .unwrap()
        .unwrap()
        .read("x")
        .unwrap()
        .unwrap();
    assert!(text.contains("v4 text"), "every edit landed: {text}");
    let (nodes, _) = kernel.branch_path(&b).unwrap();
    let e = ok_tool_results(&nodes)
        .into_iter()
        .find(|(id, _)| id == "e");
    assert!(e.is_some(), "the direct edit after the scripts passed CAS");
    assert!(
        update_notices(&nodes).is_empty(),
        "the branch's own edits are not updates: {:?}",
        update_notices(&nodes)
    );
}
