//! End-to-end: the transcript address an `agent` result carries
//! (`@rumu:/sessions/<s>/<first>..<head>`) is readable with the real
//! `WorkspacePlugin` over a real kernel.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{Plugin, ToolCall, ToolSpec, Workspaces};
use rumu_kernel::{Kernel, LlmResponse, ScriptedLlm};
use rumu_plugins::WorkspacePlugin;
use rumu_store::Store;
use serde_json::{Value, json};

/// A minimal "other" tool so the agent batch answers more than one call.
struct OtherPlugin;

#[async_trait]
impl Plugin for OtherPlugin {
    fn manifest(&self) -> rumu_core::Manifest {
        rumu_core::Manifest {
            name: "other".into(),
            tools: vec![ToolSpec {
                name: "other".into(),
                description: "test tool".into(),
                schema: json!({ "type": "object", "properties": {} }),
                background: false,
                output_schema: None,
                parallel: false,
            }],
            hooks: vec![],
            ..Default::default()
        }
    }

    async fn call_tool(
        &self,
        name: &str,
        _args: Value,
        _cx: &rumu_core::Cx,
    ) -> rumu_core::Result<rumu_core::ToolOutput> {
        Ok(rumu_core::ToolOutput::ok(format!("{name}-done")))
    }
}

#[tokio::test]
async fn agent_transcript_address_is_readable() {
    let home = tempfile::tempdir().unwrap();
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(OtherPlugin),
        Arc::new(WorkspacePlugin::new(
            home.path(),
            rumu_cards::Decks::load(home.path()).unwrap(),
            rumu_core::Workspaces::default(),
        )),
    ];
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(ScriptedLlm::strict([
            // Parent: agent + other in one batch.
            LlmResponse::calls([
                ToolCall::new("e1", "agent", json!({ "task": "find the config file" })),
                ToolCall::new("o1", "other", json!({})),
            ]),
            // Child: one tool call, then a plain reply ends it.
            LlmResponse::calls([ToolCall::new("k1", "other", json!({ "detail": true }))]),
            LlmResponse::text("config lives in /etc/rumu"),
            // Parent final.
            LlmResponse::text("all done"),
        ])),
        plugins,
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.active_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "look around").await.unwrap();
    loop {
        let v = rx.recv().await.expect("pushes");
        if v["method"] == "turn.done" && v["params"]["branch"].as_str() == Some(branch.0.as_str()) {
            break;
        }
    }

    // The agent result carries the transcript address.
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let e1 = nodes
        .iter()
        .find_map(|n| match &n.body {
            rumu_core::NodeBody::ToolResult {
                call_id, content, ..
            } if call_id == "e1" => Some(content.text.clone()),
            _ => None,
        })
        .unwrap();
    let addr = e1
        .split("[transcript: ")
        .nth(1)
        .unwrap()
        .trim_end_matches(']')
        .to_owned();
    assert!(addr.starts_with("@rumu:/sessions/"), "{addr}");

    // `read` opens it: the child's own nodes in root→head order — the
    // sub-agent prompt (its first own node), the skipped sibling, the
    // child's tool call + result, and the final reply.
    let ws = WorkspacePlugin::new(
        home.path(),
        rumu_cards::Decks::load(home.path()).unwrap(),
        rumu_core::Workspaces::default(),
    );
    let cx = rumu_core::Cx::new(
        session.id.clone(),
        branch.clone(),
        None,
        Some("test".into()),
        0,
        kernel.runtime(),
    );
    let out = ws
        .call_tool("read", json!({ "path": addr }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let elements: Vec<&str> = out
        .content
        .text
        .lines()
        .filter(|l| l.starts_with("<node "))
        .collect();
    assert_eq!(elements.len(), 5, "{}", out.content.text);
    assert!(
        out.content
            .text
            .contains("You are now a sub-agent. Goal: find the config file"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("(skipped in sub-agent branch)"),
        "{}",
        out.content.text
    );
    assert!(
        out.content
            .text
            .contains("<call id=\"k1\" name=\"other\">{\"detail\":true}</call>"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("other-done"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("config lives in /etc/rumu"),
        "{}",
        out.content.text
    );

    // The same session's overview lists both branches with live ranges.
    let sid = session.id.0.as_str();
    let out = ws
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/sessions/{sid}") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .contains(&format!("main     active     @rumu:/sessions/{sid}/")),
        "{}",
        out.content.text
    );
    assert!(
        out.content
            .text
            .contains(&format!("agent    done       @rumu:/sessions/{sid}/"))
            && out.content.text.contains("task: find the config file"),
        "{}",
        out.content.text
    );

    // And the session search finds the child's reply through the address.
    let out = ws
        .call_tool(
            "search",
            json!({ "query": "/etc/rumu", "scope": format!("@rumu:/sessions/{sid}") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.content.text.contains("config lives in /etc/rumu"),
        "{}",
        out.content.text
    );
}
