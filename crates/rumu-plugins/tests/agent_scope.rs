//! A scoped sub-agent cannot address an out-of-scope workspace: the kernel's
//! `workspace.scope` marker filters `PathState.workspaces`, and every
//! workspace tool path resolves through that state.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{BranchId, Content, Node, NodeBody, ToolCall, ToolSpec, Workspace, Workspaces};
use rumu_kernel::{Kernel, Llm, LlmResponse, OnDelta};
use rumu_plugins::WorkspacePlugin;
use rumu_store::Store;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Answers by inspecting the projection: the parent first asks for a scoped
/// sub-agent, then the child tries one out-of-scope read and gives up.
struct ScopedLlm;

/// A `context: "none"` child sees its task as a user node with this framing.
fn is_child(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        matches!(&n.body, NodeBody::User { content }
            if content.text.contains("You are now a sub-agent"))
    })
}

#[async_trait]
impl Llm for ScopedLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        if is_child(nodes) {
            let called = nodes.iter().any(|n| {
                matches!(&n.body, NodeBody::Assistant { tool_calls, .. } if !tool_calls.is_empty())
            });
            if called {
                return Ok(LlmResponse::text("could not read it"));
            }
            return Ok(LlmResponse::calls([ToolCall::new(
                "r1",
                "read",
                json!({ "path": "other:secret.txt" }),
            )]));
        }
        let started = nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Assistant { tool_calls, .. } if !tool_calls.is_empty()),
        );
        if started {
            return Ok(LlmResponse::text("done"));
        }
        Ok(LlmResponse::calls([ToolCall::new(
            "a1",
            "agent",
            json!({
                "task": "read the other workspace",
                "inherit": { "context": "none", "workspaces": ["proj"] }
            }),
        )]))
    }
}

async fn wait_turn(rx: &mut mpsc::UnboundedReceiver<Value>, branch: &str) -> Value {
    while let Some(v) = rx.recv().await {
        let m = v["method"].as_str().unwrap_or_default();
        if (m == "turn.done" || m == "turn.error") && v["params"]["branch"].as_str() == Some(branch)
        {
            return v;
        }
    }
    panic!("push stream ended before turn finished");
}

#[tokio::test]
async fn scoped_child_cannot_read_an_out_of_scope_workspace() {
    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    std::fs::write(other.path().join("secret.txt"), "top secret").unwrap();
    let workspaces = Workspaces::new(vec![
        Workspace {
            name: "proj".into(),
            ssh: None,
            root: proj.path().to_string_lossy().into_owned().into(),
            read_only: false,
        },
        Workspace {
            name: "other".into(),
            ssh: None,
            root: other.path().to_string_lossy().into_owned().into(),
            read_only: false,
        },
    ]);

    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(ScopedLlm),
        vec![Arc::new(WorkspacePlugin::new(
            home.path(),
            rumu_cards::Decks::load(home.path()).unwrap(),
            rumu_core::Workspaces::default(),
        ))],
        workspaces,
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.active_branch.0.as_str());
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    let done = wait_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    let child = kernel
        .session_tree(&session.id)
        .unwrap()
        .2
        .into_iter()
        .find(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        .expect("agent branch");
    let (nodes, state) = kernel.branch_path(&child.id).unwrap();

    // The scope filtered the child's workspaces to `proj` (+ `@scratch`).
    assert_eq!(
        state
            .workspaces
            .iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>(),
        ["proj", "@scratch"]
    );

    // `read other:secret.txt` failed at address resolution.
    let result = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::ToolResult {
                call_id,
                content,
                is_error,
                ..
            } if call_id == "r1" => Some((content_text(content), *is_error)),
            _ => None,
        })
        .expect("read result on the child");
    assert!(result.1, "{}", result.0);
    assert!(
        result.0.contains("unknown workspace `other`"),
        "{}",
        result.0
    );
    assert!(
        result.0.contains("available: proj, @scratch, @rumu"),
        "{}",
        result.0
    );
}

fn content_text(content: &Content) -> String {
    content.text.clone()
}
