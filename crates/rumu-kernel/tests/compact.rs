//! Compaction through the kernel: automatic, on overflow, by hand, and the
//! once-per-view notes plugins put back after it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rumu_core::{
    BranchId, Compaction, Content, Cx, Event, HookOutput, HookSpec, Manifest, ModelPick, Node,
    NodeBody, Plugin, Structured, ToolSpec, Usage, Workspaces, compaction_of,
};
use rumu_kernel::{Budget, Kernel, Llm, LlmError, LlmResponse, OnDelta};
use rumu_store::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

/// Replies in order, recording each request; reports a fixed window.
struct Scripted {
    replies: Mutex<VecDeque<Result<LlmResponse, LlmError>>>,
    seen: Mutex<Vec<(String, Vec<Node>)>>,
    context: Option<u64>,
}

impl Scripted {
    fn new(context: Option<u64>, replies: Vec<Result<LlmResponse, LlmError>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(vec![]),
            context,
        })
    }

    /// `(system, nodes)` of every request so far.
    fn seen(&self) -> Vec<(String, Vec<Node>)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl Llm for Scripted {
    async fn complete(
        &self,
        _pick: ModelPick<'_>,
        system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> Result<LlmResponse, LlmError> {
        self.seen
            .lock()
            .unwrap()
            .push((system.to_owned(), nodes.to_vec()));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("an unscripted model call")
    }

    fn budget(&self, _pick: ModelPick<'_>) -> Budget {
        Budget {
            context: self.context,
            ..Budget::default()
        }
    }
}

fn reply(text: &str, prompt_tokens: u64) -> Result<LlmResponse, LlmError> {
    Ok(LlmResponse {
        usage: Usage {
            prompt_tokens,
            ..Usage::default()
        },
        ..LlmResponse::text(text)
    })
}

fn summary() -> Result<LlmResponse, LlmError> {
    reply(
        &format!("<summary>\n## Goal\n{}\n</summary>", "g".repeat(200)),
        0,
    )
}

fn overflow() -> Result<LlmResponse, LlmError> {
    Err(LlmError::Request(
        "HTTP error.\nStatus: 400 Bad Request\nBody:\nprompt is too long: 210000 tokens > 200000 maximum"
            .into(),
    ))
}

/// Injects one `Seen` note per view on `llm.pre`.
struct Once;

#[derive(Serialize, Deserialize)]
struct Seen;

impl Structured for Seen {
    const TAG: &'static str = "test.seen";
    const V: u32 = 1;
}

#[async_trait]
impl Plugin for Once {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "once".into(),
            instructions: None,
            tools: vec![],
            hooks: vec![HookSpec {
                id: "once".into(),
                matcher: json!({ "event": "llm.pre" }),
                priority: 0,
            }],
        }
    }

    async fn on_hook(&self, _id: &str, _ev: &Event, cx: &Cx) -> rumu_core::Result<HookOutput> {
        if !cx.index()?.all::<Seen>().is_empty() {
            return Ok(HookOutput::default());
        }
        Ok(HookOutput {
            effects: vec![Content::note_with("intro", &Seen)],
            ..HookOutput::default()
        })
    }
}

async fn setup(llm: Arc<Scripted>) -> (Arc<Kernel>, BranchId, UnboundedReceiver<Value>) {
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm,
        vec![Arc::new(Once)],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    let rx = kernel.tap_pushes();
    (kernel, branch, rx)
}

async fn wait_turn(rx: &mut UnboundedReceiver<Value>) -> Value {
    while let Some(v) = rx.recv().await {
        let m = v["method"].as_str().unwrap_or_default();
        if m == "turn.done" || m == "turn.error" {
            return v;
        }
    }
    panic!("pushes ended before the turn did");
}

async fn turn(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    rx: &mut UnboundedReceiver<Value>,
    text: &str,
) -> Value {
    kernel.prompt(branch, text).await.unwrap();
    wait_turn(rx).await
}

fn texts(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::User { content } | NodeBody::Assistant { content, .. } => {
                Some(content.text.clone())
            }
            NodeBody::Note { .. } if compaction_of(n).is_some() => Some("<compaction>".into()),
            NodeBody::Note { content, .. } if !content.text.is_empty() => {
                Some(content.text.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_outgrown_view_is_compacted_before_the_next_call() {
    let long = "a".repeat(24_000); // 6k tokens: the tail
    // Window 50k compacts at 17k, keeping 5k.
    let llm = Scripted::new(
        Some(50_000),
        vec![reply(&long, 20_000), summary(), reply("done", 0)],
    );
    let (kernel, branch, mut rx) = setup(llm.clone()).await;
    assert_eq!(
        turn(&kernel, &branch, &mut rx, "first").await["method"],
        "turn.done"
    );
    assert_eq!(
        turn(&kernel, &branch, &mut rx, "second").await["method"],
        "turn.done"
    );

    let seen = llm.seen();
    assert_eq!(seen.len(), 3);
    // The summary was asked for in context: the branch's own view, then
    // the instruction, keeping the long reply verbatim.
    let (_, ask) = &seen[1];
    assert_eq!(
        texts(&ask[..ask.len() - 1]),
        ["first", "intro", &long, "second"]
    );
    let instruction = texts(&ask[ask.len() - 1..]).remove(0);
    assert!(
        instruction.contains("<summary>") && instruction.contains("begins \"aaaa"),
        "{instruction}"
    );

    // The next call sees the summary, the tail and the intro again.
    let (_, after) = &seen[2];
    assert_eq!(texts(after), ["<compaction>", &long, "second", "intro"]);
    let NodeBody::Note { content, .. } = &after[0].body else {
        unreachable!()
    };
    assert!(
        content.text.contains("<message>\nfirst\n</message>"),
        "{}",
        content.text
    );
    assert!(content.text.contains("@rumu:/sessions/"));
    assert!(content.text.contains("ggg"));
    let data = content.decode::<Compaction>().unwrap();
    assert_eq!(data.keep_from, Some(after[1].id.clone()));
    assert!(
        data.before >= 20_000 && data.after < data.before,
        "{data:?}"
    );

    // State still folds the whole path.
    let (path, state) = kernel.branch_path(&branch).unwrap();
    assert_eq!(state.assistant, "assistant");
    assert!(texts(&path).contains(&"first".to_owned()));
}

#[tokio::test]
async fn an_overflow_compacts_and_retries_once() {
    let long = "b".repeat(40_000);
    let llm = Scripted::new(
        None,
        vec![reply(&long, 0), overflow(), summary(), reply("fits", 0)],
    );
    let (kernel, branch, mut rx) = setup(llm.clone()).await;
    turn(&kernel, &branch, &mut rx, "first").await;
    assert_eq!(
        turn(&kernel, &branch, &mut rx, "second").await["method"],
        "turn.done"
    );

    let seen = llm.seen();
    // Standalone: the summarizer's own prompt over a transcript.
    let (system, ask) = &seen[2];
    assert!(system.contains("You summarize a conversation"));
    assert!(texts(ask)[0].contains("<conversation>\n"));
    assert_eq!(texts(&seen[3].1)[0], "<compaction>");
    let (path, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(texts(&path).last().unwrap(), "fits");
}

#[tokio::test]
async fn a_second_overflow_fails_the_turn() {
    let long = "b".repeat(40_000);
    let llm = Scripted::new(
        None,
        vec![reply(&long, 0), overflow(), summary(), overflow()],
    );
    let (kernel, branch, mut rx) = setup(llm.clone()).await;
    turn(&kernel, &branch, &mut rx, "first").await;
    let done = turn(&kernel, &branch, &mut rx, "second").await;
    assert_eq!(done["method"], "turn.error");
    assert!(
        done["params"]["message"]
            .as_str()
            .unwrap()
            .contains("prompt is too long")
    );
}

#[tokio::test]
async fn compacting_by_hand_keeps_the_focus() {
    // Short: the tail is held to a third of it.
    let llm = Scripted::new(None, vec![reply("one", 0), reply("two", 0), summary()]);
    let (kernel, branch, mut rx) = setup(llm.clone()).await;
    turn(&kernel, &branch, &mut rx, "first").await;
    turn(&kernel, &branch, &mut rx, "second").await;
    kernel
        .compact(&branch, Some("the parser bug".into()))
        .await
        .unwrap();
    assert_eq!(wait_turn(&mut rx).await["method"], "turn.done");

    let seen = llm.seen();
    let (_, ask) = seen.last().unwrap();
    let instruction = texts(&ask[ask.len() - 1..]).remove(0);
    assert!(instruction.contains("<focus>\nthe parser bug\n</focus>"));
    let (path, _) = kernel.branch_path(&branch).unwrap();
    let data = compaction_of(path.last().unwrap()).unwrap();
    assert_eq!(data.reason, rumu_core::CompactReason::Manual);
    assert!(data.keep_from.is_some());
}
