//! `session.create` + `session.wait` end to end: the real kernel and
//! plugins, an LLM that answers by the assistant it runs as. scout starts a
//! session of ledger; the user approves through a decision, ledger does the
//! task from its own library, keeping a card of its own, and scout waits for
//! its reply.

use std::sync::Arc;

use async_trait::async_trait;
use rumu_core::{BranchId, Node, NodeBody, ToolCall, ToolSpec};
use rumu_kernel::{Kernel, Llm, LlmResponse, OnDelta};
use rumu_store::Store;
use serde_json::{Value, json};
use tokio::sync::mpsc;

/// Plays both assistants. Whichever runs is read off the first user message: the
/// started session's prompt starts with "The assistant `scout` sends this".
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

fn results(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .filter(|n| matches!(n.body, NodeBody::ToolResult { .. }))
        .count()
}

/// The starter's side: start a session with `create`'s arguments, wait on it,
/// then report what came back (or the refusal).
fn starter(nodes: &[Node], create: Value) -> LlmResponse {
    let result = last_result(nodes);
    match (results(nodes), result) {
        (0, _) => LlmResponse::calls([ToolCall::new(
            "c1",
            "invoke",
            json!({ "action": "session.create", "path": "@rumu:/sessions", "args": create }),
        )]),
        (1, Some(started)) if started.starts_with("started session") => {
            let path = started.split('`').nth(1).unwrap().to_owned();
            LlmResponse::calls([ToolCall::new(
                "w1",
                "invoke",
                json!({ "action": "session.wait", "path": path }),
            )])
        }
        (_, result) => LlmResponse::text(format!("scout heard: {}", result.unwrap_or_default())),
    }
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
        if prompt.starts_with("The assistant `scout` sends this") {
            // ledger: keep a card of its own, then answer from the card
            // recall gave it.
            return Ok(match last_result(nodes) {
                None => LlmResponse::calls([ToolCall::new(
                    "w1",
                    "write",
                    json!({ "path": "@rumu:/cards/self/noted", "content": "asked about the budget" }),
                )]),
                Some(write) => {
                    let recalled = nodes.iter().any(|n| matches!(&n.body,
                        NodeBody::Note { content, .. } if content.text.contains("monthly budget is 300")));
                    LlmResponse::text(format!("budget 300 (recalled: {recalled}; write: {write})"))
                }
            });
        }
        Ok(starter(nodes, task()))
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

/// The `ledger` deck id written by [`home`]; `session.create` names decks by id.
const LEDGER: &str = "0193a2f1-0000-7000-8000-000000000003";

/// scout's `session.create` arguments for ledger.
fn task() -> Value {
    json!({ "assistant": LEDGER, "prompt": "find the monthly budget" })
}

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
    let decks = rumu_cards::Decks::load(home).unwrap();
    let plugins = rumu_plugins::PluginSet::new(
        home,
        decks,
        Default::default(),
        rumu_plugins::McpHub::new(home),
        Arc::new(rumu_plugins::Skills::with_dirs(home, Vec::new(), None)),
        &[],
    );
    Kernel::new(
        Arc::new(Store::in_memory()),
        llm,
        plugins.all(),
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
    let branch = session.main_branch.clone();
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
async fn an_approved_task_is_done_by_the_other_assistant() {
    let home = home();
    let kernel = kernel(home.path()).await;

    let (reply, asked) = scout_turn(&kernel, home.path(), json!({ "allow": true })).await;
    assert_eq!(asked, 1);
    assert!(reply.contains("of `ledger` is idle"), "{reply}");
    assert!(reply.contains("budget 300"), "{reply}");
    // Outside any project ledger's session is its own: scout gets the reply
    // it waited for, not the transcript.
    assert!(!reply.contains("[transcript:"), "{reply}");
    // ledger worked from its own library…
    assert!(reply.contains("recalled: true"), "{reply}");
    // …and kept a card of its own.
    let ledger_deck = rumu_cards::Decks::load(home.path()).unwrap();
    let ledger_deck = ledger_deck.get(LEDGER).unwrap();
    assert!(ledger_deck.read("noted").unwrap().is_some(), "{reply}");

    // The task ran in a session of ledger.
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
async fn a_declined_task_never_reaches_the_other_assistant() {
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
    let branch = session.main_branch.clone();
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

/// scout starts a session of its own: `session.create` without `assistant`.
struct SelfLlm;

/// A session started from one in a project is in it too, at the point the
/// starter has it (so the paths its prompt names resolve there); a project
/// deck the starter only borrows stays the starter's.
#[tokio::test]
async fn a_started_session_has_the_starters_project_where_it_has_it() {
    let home = home();
    let (eval, web) = (
        "0193a2f1-0000-7000-8000-000000000004",
        "0193a2f1-0000-7000-8000-000000000005",
    );
    create_deck(
        home.path(),
        &format!("id: {eval}\nname: eval\nproject: true\n"),
        &[],
    );
    create_deck(
        home.path(),
        &format!("id: {web}\nname: Web App\nproject: true\n"),
        &[],
    );
    let kernel = kernel_on(home.path(), Arc::new(SelfLlm)).await;
    let scout = deck_id(home.path(), "scout");
    let (session, _) = kernel
        .create_rooted_session(rumu_core::SessionRoot {
            assistant: scout.clone(),
            mounts: vec![
                rumu_core::CardMount::new(eval, "/cards/project", true),
                rumu_core::CardMount::new(web, "/cards/web-app", false),
            ],
        })
        .await
        .unwrap();
    assert_eq!(session.project.as_deref(), Some(eval));
    let branch = session.main_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "ask myself").await.unwrap();
    let (done, _) = run(&kernel, &mut rx, &branch, json!({ "allow": false })).await;
    assert_eq!(done["method"], "turn.done", "{done}");
    let started = kernel
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.id != session.id)
        .expect("the started session");
    assert_eq!(started.project.as_deref(), Some(eval));
    let (_, st) = kernel.branch_path(&started.main_branch).unwrap();
    assert_eq!(
        st.project_mount().map(|m| m.at.as_str()),
        Some("/cards/project")
    );
    assert!(st.mounts.iter().all(|m| m.deck != web));
}

#[async_trait]
impl Llm for SelfLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        _on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        if first_user(nodes).starts_with("You sent this to yourself") {
            return Ok(LlmResponse::text("done on my own"));
        }
        Ok(starter(nodes, json!({ "prompt": "tidy up" })))
    }
}

#[tokio::test]
async fn a_session_of_your_own_starts_unasked() {
    let home = home();
    let kernel = kernel_on(home.path(), Arc::new(SelfLlm)).await;
    let (reply, asked) = scout_turn(&kernel, home.path(), json!({ "allow": false })).await;
    assert_eq!(asked, 0);
    assert!(reply.contains("of `scout` is idle"), "{reply}");
    assert!(reply.contains("done on my own"), "{reply}");
    // Your own session you can see.
    assert!(reply.contains("[transcript: @rumu:/sessions/"), "{reply}");
    let scout = deck_id(home.path(), "scout");
    let sessions = kernel.list_sessions().unwrap();
    assert_eq!(
        sessions.iter().filter(|s| s.assistant == scout).count(),
        2,
        "the starting session and the started one"
    );
}

/// scout starts ledger's session and waits; ledger thinks forever.
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
        if first_user(nodes).starts_with("The assistant `scout` sends this") {
            return std::future::pending().await;
        }
        Ok(starter(nodes, task()))
    }
}

#[tokio::test]
async fn a_started_session_outlives_the_turn_that_started_it() {
    let home = home();
    let kernel = kernel_on(home.path(), Arc::new(StuckLlm)).await;
    let scout = deck_id(home.path(), "scout");
    let (session, _) = kernel.create_session(scout).await.unwrap();
    let scout = session.main_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&scout, "ask ledger").await.unwrap();
    let mut ledger = None;
    let finished = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(v) = rx.recv().await {
            let params = &v["params"];
            let branch = params["branch"].as_str();
            match v["method"].as_str().unwrap_or_default() {
                "decision.requested" => kernel
                    .answer_decision(params["id"].as_str().unwrap(), json!({ "allow": true }))
                    .unwrap(),
                "turn.started" if branch != Some(&scout.0) => {
                    ledger = branch.map(str::to_owned);
                    kernel.abort(&scout).unwrap();
                }
                "turn.done" | "turn.error" if branch == Some(&scout.0) => return,
                _ => {}
            }
        }
    })
    .await;
    assert!(finished.is_ok(), "scout's turn never finished");
    // Stopping scout stopped only its wait: ledger works on.
    let ledger = BranchId::from(ledger.expect("ledger's turn started").as_str());
    assert!(kernel.is_running(&ledger));
    kernel.stop(&ledger).unwrap();
}
