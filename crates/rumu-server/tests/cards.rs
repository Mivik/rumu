//! End-to-end card scenarios over a real kernel + the memory & workspace
//! plugins (scripted LLM): only branches that actually read a card get
//! update notices; old snapshots survive card edits (history is never
//! rewritten); forking before a read drops the notice for the new branch.

use std::sync::Arc;

use rumu_core::{Node, NodeBody, Plugin, ToolCall, WorkspaceCatalog};
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
            if content.decode::<rumu_cards::usage::CardUse>().is_some_and(|u| u.id == "x"))
    })
}

/// A temp RUMU_HOME with one card `x` and no rules. Branch A's use comes
/// purely from a card read, so branch B (which never reads) has no use record.
fn card_home() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    std::fs::create_dir_all(home.join("cards")).unwrap();
    std::fs::write(home.join("cards/x.md"), "---\ntitle: X\n---\nv1 body\n").unwrap();
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
    //   A turn 1: read(@rumu:/cards/x) + reply;  B turn 1: reply;  A turn 2: reply;
    //   fork turn: reply.
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(MemoryPlugin::new(&home)),
        Arc::new(WorkspacePlugin::new(&home, WorkspaceCatalog::default())),
    ];
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(ScriptedLlm::strict([
            resp_call("a1", "read", json!({ "path": "@rumu:/cards/x" })),
            LlmResponse::text("done-a1"),
            LlmResponse::text("done-b1"),
            LlmResponse::text("done-a2"),
            LlmResponse::text("done-fork"),
        ])),
        plugins,
        WorkspaceCatalog {
            default: Some("@rumu".into()),
            ..WorkspaceCatalog::default()
        },
    )
    .unwrap();

    let (a, _) = kernel.create_session(None).await.unwrap();
    let ba = a.active_branch.clone();
    run_turn(&kernel, &ba, "read card x").await;
    let (nodes_a, _) = kernel.branch_path(&ba).unwrap();

    // The read returned the v1 snapshot and is recorded on the path.
    let results = ok_tool_results(&nodes_a);
    assert_eq!(results.len(), 1);
    assert!(results[0].1.contains("v1 body"));
    assert!(has_card_use(&nodes_a), "card_use marker on A's path");

    // The card changes on disk.
    std::fs::write(home.join("cards/x.md"), "---\ntitle: X\n---\nv2 body\n").unwrap();

    // B (fresh session, same home) never read the card → no notice.
    let (b, _) = kernel.create_session(None).await.unwrap();
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
        updates
            .iter()
            .filter(|u| u.id == "x" && u.rev != "deleted")
            .count(),
        1,
        "exactly one notice: {updates:?}"
    );
    let notice_text = nodes_a2
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Note { content, .. }
                if content
                    .decode::<rumu_cards::usage::CardUpdate>()
                    .is_some_and(|u| u.id == "x") =>
            {
                Some(content.text.clone())
            }
            _ => None,
        })
        .unwrap();
    assert!(notice_text.contains("`@rumu:/cards/x`"), "{notice_text}");
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
