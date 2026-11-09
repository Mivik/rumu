//! `ask` end to end: the real kernel and plugins, an LLM that answers by the
//! assistant it runs as. scout asks ledger; the user approves through a decision,
//! and ledger answers from its own library in a session of its own.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{BranchId, Node, NodeBody, ToolCall, ToolSpec};
use rumu_kernel::{Kernel, Llm, LlmResponse, OnDelta};
use rumu_plugins::load_config;
use rumu_store::Store;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Plays both assistants. Whichever runs is read off the first user message: the
/// answering session's prompt starts with "The assistant `scout` asks you".
struct AssistantsLlm;

fn first_user(nodes: &[Node]) -> String {
    nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::User { content } => Some(content.text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

fn last_result(nodes: &[Node]) -> Option<String> {
    nodes.iter().rev().find_map(|n| match &n.body {
        NodeBody::ToolResult { content, .. } => Some(content.text.clone()),
        _ => None,
    })
}

#[async_trait]
impl Llm for AssistantsLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        let prompt = first_user(nodes);
        let write = |path: &str| {
            LlmResponse::calls([ToolCall::new(
                "w1",
                "write",
                json!({ "path": path, "content": "x" }),
            )])
        };
        if prompt.contains("You are now a sub-agent") {
            // A clean-context sub-agent of ledger tries the write itself.
            return Ok(match last_result(nodes) {
                None => write("@rumu:/cards/leak-sub"),
                Some(refusal) => LlmResponse::text(format!("sub-agent write: {refusal}")),
            });
        }
        if prompt.starts_with("The assistant `scout` asks you") {
            // ledger: try to write a card (refused while answering), have a
            // sub-agent try, then answer from the card recall gave it.
            let results = nodes
                .iter()
                .filter(|n| matches!(n.body, NodeBody::ToolResult { .. }))
                .count();
            return Ok(match results {
                0 => write("@rumu:/cards/leak"),
                1 => LlmResponse::calls([ToolCall::new(
                    "s1",
                    "agent",
                    json!({ "task": "save a card", "inherit": { "context": "none" } }),
                )]),
                _ => {
                    let refusal = last_result(nodes).unwrap_or_default();
                    let recalled = nodes.iter().any(|n| matches!(&n.body,
                        NodeBody::Note { content, .. } if content.text.contains("monthly budget is 300")));
                    LlmResponse::text(format!(
                        "budget 300 (recalled: {recalled}; write: {refusal})"
                    ))
                }
            });
        }
        // scout: ask once, then report what came back.
        Ok(match last_result(nodes) {
            None => LlmResponse::calls([ToolCall::new(
                "a1",
                "ask",
                json!({ "to": LEDGER, "questions": [{ "question": "what is the monthly budget?" }] }),
            )]),
            Some(result) => LlmResponse::text(format!("scout heard: {result}")),
        })
    }
}

/// The deck id of a named deck.
fn deck_id(home: &std::path::Path, name: &str) -> String {
    rumu_cards::Decks::load(home)
        .unwrap()
        .list()
        .into_iter()
        .find(|d| d.meta.name == name)
        .unwrap()
        .id()
        .to_owned()
}

/// The `ledger` deck id written by [`home`]; `ask` names decks by id.
const LEDGER: &str = "0193a2f1-0000-7000-8000-000000000003";

fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let finance_id = "0193a2f1-0000-7000-8000-000000000001";
    create_deck(
        home.path(),
        &format!("id: {finance_id}\nname: finance\n"),
        &[(
            "budget",
            "---\ndescription: the budget\nkeywords: [budget]\n---\nThe monthly budget is 300.\n",
        )],
    );
    for (name, id) in [
        ("scout", "0193a2f1-0000-7000-8000-000000000002"),
        ("ledger", "0193a2f1-0000-7000-8000-000000000003"),
    ] {
        let mounts = if name == "ledger" {
            format!("mounts:\n  - {{ deck: {finance_id}, at: /cards/finance }}\n")
        } else {
            String::new()
        };
        create_deck(
            home.path(),
            &format!("id: {id}\nname: {name}\nassistant: true\n{mounts}"),
            &[],
        );
    }
    home
}

/// Create a bare deck with the given `deck.yaml` and cards, through the
/// deck's committing write primitive.
fn create_deck(home: &std::path::Path, text: &str, cards: &[(&str, &str)]) {
    let meta = rumu_cards::yaml::from_str::<rumu_cards::DeckMeta>(text).unwrap();
    let dir = home.join("decks").join(&meta.id);
    rumu_cards::git::repo(&dir).init().unwrap();
    let deck = rumu_cards::Deck { meta, dir };
    deck.commit(
        &rumu_cards::Author::user(),
        "create deck",
        &std::collections::BTreeMap::from([("deck.yaml".to_owned(), Some(text.to_owned()))]),
    )
    .unwrap();
    for (path, text) in cards {
        deck.write_file(path, text, &rumu_cards::Author::user())
            .unwrap();
    }
}

async fn kernel(home: &std::path::Path) -> Arc<Kernel> {
    kernel_on(home, Arc::new(AssistantsLlm)).await
}

async fn kernel_on(home: &std::path::Path, llm: Arc<dyn Llm>) -> Arc<Kernel> {
    let config = load_config(home).unwrap();
    let decks = rumu_cards::Decks::load(home).unwrap();
    let plugins = rumu_plugins::load_all(home, &config, decks, Default::default(), &[])
        .await
        .unwrap();
    Kernel::new(
        Arc::new(Store::in_memory()),
        llm,
        plugins,
        Default::default(),
    )
    .unwrap()
}

/// Drive pushes until `branch` finishes, answering every decision with
/// `answer` (counting them).
async fn run(
    kernel: &Arc<Kernel>,
    rx: &mut mpsc::UnboundedReceiver<Value>,
    branch: &BranchId,
    answer: Value,
) -> (Value, usize) {
    let mut asked = 0;
    while let Some(v) = rx.recv().await {
        match v["method"].as_str().unwrap_or_default() {
            "decision.requested" => {
                asked += 1;
                let request = &v["params"]["request"];
                assert_eq!(request["kind"], "ask");
                assert_eq!(
                    (request["from"].as_str(), request["to"].as_str()),
                    (Some("scout"), Some("ledger"))
                );
                kernel
                    .answer_decision(v["params"]["id"].as_str().unwrap(), answer.clone())
                    .unwrap();
            }
            "turn.done" | "turn.error" if v["params"]["branch"].as_str() == Some(&branch.0) => {
                return (v, asked);
            }
            _ => {}
        }
    }
    panic!("push stream ended before the turn finished");
}

async fn scout_turn(
    kernel: &Arc<Kernel>,
    home: &std::path::Path,
    answer: Value,
) -> (String, usize) {
    let scout = deck_id(home, "scout");
    let (session, _) = kernel.create_session(scout).await.unwrap();
    let branch = session.active_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "ask ledger").await.unwrap();
    let (done, asked) = run(kernel, &mut rx, &branch, answer).await;
    assert_eq!(done["method"], "turn.done", "{done}");
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let reply = nodes
        .iter()
        .rev()
        .find_map(|n| match &n.body {
            NodeBody::Assistant {
                content,
                tool_calls,
                ..
            } if tool_calls.is_empty() => Some(content.text.clone()),
            _ => None,
        })
        .unwrap();
    (reply, asked)
}

#[tokio::test]
async fn approved_ask_is_answered_by_the_other_assistant() {
    let home = home();
    let kernel = kernel(home.path()).await;

    let (reply, asked) = scout_turn(&kernel, home.path(), json!({ "allow": true })).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("`ledger` answers"), "{reply}");
    assert!(reply.contains("budget 300"), "{reply}");
    // ledger answered from its own library…
    assert!(reply.contains("recalled: true"), "{reply}");
    // …and could not write while answering.
    assert!(reply.contains("cards cannot change here"), "{reply}");
    assert!(
        rumu_cards::open_deck(home.path(), "finance")
            .unwrap()
            .unwrap()
            .read("leak")
            .unwrap()
            .is_none()
    );
    // Nor could a clean-context sub-agent of it.
    assert!(reply.contains("sub-agent write"), "{reply}");
    assert!(
        rumu_cards::open_deck(home.path(), "finance")
            .unwrap()
            .unwrap()
            .read("leak-sub")
            .unwrap()
            .is_none()
    );

    // The answer ran as a session of ledger.
    let sessions = kernel.list_sessions().unwrap();
    let ledger = deck_id(home.path(), "ledger");
    let assistants: Vec<&str> = sessions.iter().map(|s| s.assistant.as_str()).collect();
    assert!(assistants.contains(&ledger.as_str()), "{assistants:?}");
    assert!(kernel.decisions().is_empty());

    // Without "always", the next ask prompts again.
    let (_, asked) = scout_turn(&kernel, home.path(), json!({ "allow": true })).await;
    assert_eq!(asked, 1);
}

#[tokio::test]
async fn declined_ask_never_reaches_the_other_assistant() {
    let home = home();
    let kernel = kernel(home.path()).await;
    let (reply, asked) = scout_turn(&kernel, home.path(), json!({ "allow": false })).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("declined"), "{reply}");
    let sessions = kernel.list_sessions().unwrap();
    let ledger = deck_id(home.path(), "ledger");
    assert!(sessions.iter().all(|s| s.assistant != ledger));
}

#[tokio::test]
async fn allow_for_this_session_does_not_reach_other_sessions() {
    let home = home();
    let kernel = kernel(home.path()).await;
    let (reply, asked) = scout_turn(
        &kernel,
        home.path(),
        json!({ "allow": true, "scope": "session" }),
    )
    .await;
    assert_eq!(asked, 1);
    assert!(reply.contains("budget 300"), "{reply}");
    // A new session of the same assistant asks again.
    let (_, asked) = scout_turn(&kernel, home.path(), json!({ "allow": true })).await;
    assert_eq!(asked, 1);
    assert!(!home.path().join("grants.json").exists());
}

#[tokio::test]
async fn always_allow_is_remembered_across_restarts() {
    let home = home();
    let kernel = kernel(home.path()).await;
    let (reply, asked) = scout_turn(
        &kernel,
        home.path(),
        json!({ "allow": true, "scope": "always" }),
    )
    .await;
    assert_eq!(asked, 1);
    assert!(reply.contains("budget 300"), "{reply}");
    let (_, asked) = scout_turn(&kernel, home.path(), json!({ "allow": false })).await;
    assert_eq!(asked, 0, "granted: no prompt");

    // A fresh kernel reads the grant back from grants.json.
    let kernel = self::kernel(home.path()).await;
    let (reply, asked) = scout_turn(&kernel, home.path(), json!({ "allow": false })).await;
    assert_eq!(asked, 0);
    assert!(reply.contains("budget 300"), "{reply}");
}

#[tokio::test]
async fn aborting_the_turn_cancels_the_question() {
    let home = home();
    let kernel = kernel(home.path()).await;
    let scout = deck_id(home.path(), "scout");
    let (session, _) = kernel.create_session(scout).await.unwrap();
    let branch = session.active_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "ask ledger").await.unwrap();
    while let Some(v) = rx.recv().await {
        if v["method"] == "decision.requested" {
            assert_eq!(kernel.decisions().len(), 1);
            kernel.abort(&branch).unwrap();
            break;
        }
    }
    while let Some(v) = rx.recv().await {
        if v["method"] == "decision.resolved" {
            break;
        }
    }
    assert!(kernel.decisions().is_empty());
}

/// scout asks; ledger thinks forever.
struct StuckLlm;

#[async_trait]
impl Llm for StuckLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        if first_user(nodes).starts_with("The assistant `scout` asks you") {
            return std::future::pending().await;
        }
        Ok(LlmResponse::calls([ToolCall::new(
            "a1",
            "ask",
            json!({ "to": LEDGER, "questions": [{ "question": "what is the monthly budget?" }] }),
        )]))
    }
}

#[tokio::test]
async fn aborting_the_asker_winds_the_answering_turn_down() {
    let home = home();
    let kernel = kernel_on(home.path(), Arc::new(StuckLlm)).await;
    let scout = deck_id(home.path(), "scout");
    let (session, _) = kernel.create_session(scout).await.unwrap();
    let scout = session.active_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&scout, "ask ledger").await.unwrap();
    let mut ledger = None;
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(v) = rx.recv().await {
            let params = &v["params"];
            match v["method"].as_str().unwrap_or_default() {
                "decision.requested" => kernel
                    .answer_decision(params["id"].as_str().unwrap(), json!({ "allow": true }))
                    .unwrap(),
                "turn.started" if params["reason"] == "consult" => {
                    ledger = params["branch"].as_str().map(str::to_owned);
                    kernel.abort(&scout).unwrap();
                }
                "turn.done" if params["branch"].as_str() == ledger.as_deref() => return,
                _ => {}
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "the answering turn never finished");
    // Its loop guard is gone: a new prompt on it starts a turn, not a steer.
    let ledger = BranchId::from(ledger.unwrap().as_str());
    assert!(!kernel.is_running(&ledger));
}
