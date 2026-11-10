//! Script-side sub-agents: `Promise.all` fan-out from `code`.
//!
//! Uses the real `CodePlugin` and the kernel's built-in `agent` over a
//! kernel, with a fake LLM that answers by inspecting the projection (parent
//! first call → the code script; a child's sub-agent prompt → a plain reply,
//! which ends the sub-agent; otherwise done).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use rumu_core::{
    BranchId, BranchKind, BranchStatus, Content, Node, NodeBody, SessionId, ToolCall, Workspaces,
};
use rumu_kernel::{Kernel, Llm, LlmResponse, OnDelta};
use rumu_plugins::CodePlugin;
use rumu_store::Store;
use serde_json::{Value, json};
use tokio::sync::mpsc;

const FANOUT: &str = r#"
const res = await Promise.all([
  rumu.agent({ task: "alpha" }),
  rumu.agent({ task: "beta" }),
]);
return res.join("|");
"#;

struct AgentLlm {
    /// Children overlap check: max simultaneously-active completions.
    active: AtomicUsize,
    max_active: AtomicUsize,
    child_sleep: Duration,
}

impl AgentLlm {
    fn new(child_sleep: Duration) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            child_sleep,
        }
    }
}

fn content_text(content: &Content) -> String {
    content.text.clone()
}

fn child_task(nodes: &[Node]) -> Option<String> {
    for n in nodes {
        if let NodeBody::User { content } = &n.body {
            let text = content_text(content);
            if let Some(rest) = text.split("You are now a sub-agent. Goal: ").nth(1) {
                return Some(rest.lines().next().unwrap_or("").to_owned());
            }
        }
    }
    None
}

#[async_trait]
impl Llm for AgentLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[rumu_core::ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        if let Some(goal) = child_task(nodes) {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.child_sleep).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            // A reply without tool calls ends the sub-agent; its text is
            // the summary that comes back (plus a transcript address).
            return Ok(LlmResponse::text(format!("{goal}-summary")));
        }
        let has_assistant = nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Assistant { tool_calls, .. } if !tool_calls.is_empty()),
        );
        if !has_assistant {
            let code = if self.child_sleep > Duration::from_secs(10) {
                r#"return await rumu.agent({ task: "slow" });"#
            } else {
                FANOUT
            };
            return Ok(LlmResponse::calls([ToolCall::new(
                "c1",
                "code",
                json!({ "code": code }),
            )]));
        }
        Ok(LlmResponse::text("done"))
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

async fn setup_async(llm: Arc<AgentLlm>) -> (Arc<Kernel>, SessionId, BranchId) {
    let code = Arc::new(CodePlugin::new(&[rumu_kernel::agent_tool()]));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm,
        vec![code],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.active_branch.0.as_str());
    (kernel, session.id, branch)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn script_promise_all_agents_fan_out() {
    let llm = Arc::new(AgentLlm::new(Duration::from_millis(150)));
    let (kernel, session, branch) = setup_async(llm.clone()).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "look around").await.unwrap();
    let done = wait_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    // The model sees only the single `code` result; the two sub-agents'
    // invoke results are trace nodes nested under it.
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let results: Vec<&Node> = nodes
        .iter()
        .filter(|n| matches!(&n.body, NodeBody::ToolResult { invoke: None, .. }))
        .collect();
    assert_eq!(results.len(), 1, "only the code call is model-visible");
    let text = match &results[0].body {
        NodeBody::ToolResult { content, .. } => content_text(content),
        _ => unreachable!(),
    };
    let result = text
        .split("--- result ---\n")
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    assert!(
        result.starts_with("alpha-summary\n\n[transcript: @rumu:/sessions/")
            && result.contains("beta-summary\n\n[transcript: @rumu:/sessions/"),
        "{text}"
    );

    // Both sub-agents finished, labelled with the internal call id allocated by
    // the kernel (order is completion order for concurrent agents, but each
    // branch points at its own trace node).
    let branches = kernel.session_tree(&session).unwrap().2;
    let trace: Vec<&Node> = nodes
        .iter()
        .filter(|n| {
            matches!(&n.body, NodeBody::ToolResult { invoke: Some(inv), .. } if inv.name == "agent")
        })
        .collect();
    assert_eq!(trace.len(), 2, "both invokes are trace nodes");
    let agents: Vec<_> = branches
        .iter()
        .filter(|b| matches!(b.kind, BranchKind::Agent { .. }))
        .collect();
    assert_eq!(agents.len(), 2);
    let mut ids = Vec::new();
    for b in &agents {
        assert_eq!(b.status, BranchStatus::Done);
        let BranchKind::Agent {
            task,
            call_id,
            via_plugin,
            ..
        } = &b.kind
        else {
            unreachable!()
        };
        assert!(via_plugin, "issued from the code script");
        assert!(
            trace.iter().any(|n| matches!(&n.body,
                NodeBody::ToolResult { call_id: c, content, .. }
                if c == call_id
                    && content.text.starts_with(&format!("{task}-summary")))),
            "the branch is labelled with its own invoke node"
        );
        ids.push(call_id.clone());
    }
    ids.sort();
    assert_eq!(ids, ["c1.1", "c1.2"]);
    assert!(
        llm.max_active.load(Ordering::SeqCst) >= 2,
        "the two sub-agents' LLM calls overlapped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_cancels_script_agents() {
    let llm = Arc::new(AgentLlm::new(Duration::from_secs(120)));
    let (kernel, session, branch) = setup_async(llm).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "look around").await.unwrap();
    // Wait until the sub-agent child branch exists (the script's call reached
    // the kernel), then abort the parent turn.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let branches = kernel.session_tree(&session).unwrap().2;
        if branches
            .iter()
            .any(|b| matches!(b.kind, BranchKind::Agent { .. }))
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent branch was never created"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    kernel.abort(&branch).unwrap();
    let done = wait_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let text = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::ToolResult {
                call_id, content, ..
            } if call_id == "c1" => Some(content_text(content)),
            _ => None,
        })
        .expect("code result");
    assert_eq!(text, "aborted");

    // The child branch is cancelled by its parent token and marked Done.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let branches = kernel.session_tree(&session).unwrap().2;
        if let Some(b) = branches
            .iter()
            .find(|b| matches!(b.kind, BranchKind::Agent { .. }))
            && b.status == BranchStatus::Done
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "agent branch did not finish after abort"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
