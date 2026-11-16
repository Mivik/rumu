//! `rumu acp` end to end: the real binary on a temp `RUMU_HOME` with a
//! scripted model, driven by the SDK's client.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ClientCapabilities, ContentBlock, CreateElicitationRequest,
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationCapabilities,
    ElicitationContentValue, ElicitationFormCapabilities, InitializeRequest, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SessionUpdate, StopReason, ToolCallStatus,
};
use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Agent, Client, JsonRpcRequest, JsonRpcResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How long any one exchange may take before the test fails.
const PATIENCE: Duration = Duration::from_secs(30);

/// What the client saw, in order.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
enum Seen {
    Update(SessionUpdate),
    /// A prompt answered, by its label.
    Done(&'static str),
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<Seen>>>);

impl Log {
    fn push(&self, seen: Seen) {
        self.0.lock().unwrap().push(seen);
    }

    /// The reply text so far, with each answered prompt marked `|label|`.
    fn transcript(&self) -> String {
        let mut out = String::new();
        for seen in self.0.lock().unwrap().iter() {
            match seen {
                Seen::Update(SessionUpdate::AgentMessageChunk(c)) => {
                    if let ContentBlock::Text(t) = &c.content {
                        out.push_str(&t.text);
                    }
                }
                Seen::Done(label) => out.push_str(&format!("|{label}|")),
                _ => {}
            }
        }
        out
    }

    /// The text content of the last update of tool call `id`.
    fn tool_text(&self, id: &str) -> Option<(ToolCallStatus, String)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|seen| match seen {
                Seen::Update(SessionUpdate::ToolCallUpdate(u))
                    if &*u.tool_call_id.0 == id && u.fields.status.is_some() =>
                {
                    let text = u
                        .fields
                        .content
                        .iter()
                        .flatten()
                        .filter_map(|c| match c {
                            agent_client_protocol::v1::schema::ToolCallContent::Content(c) => {
                                match &c.content {
                                    ContentBlock::Text(t) => Some(t.text.clone()),
                                    _ => None,
                                }
                            }
                            _ => None,
                        })
                        .collect();
                    Some((u.fields.status.unwrap(), text))
                }
                _ => None,
            })
    }

    fn saw_tool_call(&self, id: &str) -> bool {
        self.0.lock().unwrap().iter().any(
            |seen| matches!(seen, Seen::Update(SessionUpdate::ToolCall(t)) if &*t.tool_call_id.0 == id),
        )
    }
}

/// How the test client answers the agent's questions.
#[derive(Clone, Copy, Default)]
struct Answers {
    /// The option id picked for every permission request.
    permission: Option<&'static str>,
    /// Declares form elicitation, and accepts with `q1` set to this.
    elicit: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteerResponse)]
#[serde(rename_all = "camelCase")]
struct SteerRequest {
    session_id: String,
    prompt: Vec<ContentBlock>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    meta: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct SteerResponse {
    outcome: String,
}

/// Start `rumu acp` on a fresh home with the model script `script` (YAML),
/// open a session in a fresh directory, and run `body` with it.
async fn with_session(
    script: &str,
    answers: Answers,
    body: impl AsyncFnOnce(ConnectionTo<Agent>, String, Log),
) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let script_path = dir.path().join("script.yaml");
    std::fs::write(&script_path, script).unwrap();
    let agent = AcpAgent::new(
        AcpAgentConfig::new(env!("CARGO_BIN_EXE_rumu"))
            .arg("acp")
            .env("RUMU_HOME", home.display().to_string())
            .env("RUMU_LLM", format!("script:{}", script_path.display()))
            .env("RUST_LOG", "warn"),
    );
    let log = Log::default();
    let run = Client
        .v1()
        .on_receive_notification(
            {
                let log = log.clone();
                async move |n: SessionNotification, _cx| {
                    log.push(Seen::Update(n.update));
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |_: RequestPermissionRequest, responder, _cx| {
                let outcome = match answers.permission {
                    Some(id) => {
                        RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
                    }
                    None => RequestPermissionOutcome::Cancelled,
                };
                responder.respond(RequestPermissionResponse::new(outcome))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_: CreateElicitationRequest, responder, _cx| {
                let content = BTreeMap::from([(
                    "q1".to_owned(),
                    ElicitationContentValue::String(answers.elicit.unwrap_or_default().into()),
                )]);
                responder.respond(CreateElicitationResponse::new(ElicitationAction::Accept(
                    ElicitationAcceptAction::new().content(content),
                )))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async move |cx: ConnectionTo<Agent>| {
            let mut caps = ClientCapabilities::new();
            if answers.elicit.is_some() {
                caps = caps.elicitation(
                    ElicitationCapabilities::new().form(ElicitationFormCapabilities::new()),
                );
            }
            cx.send_request(InitializeRequest::new(ProtocolVersion::V1).client_capabilities(caps))
                .block_task()
                .await?;
            let session = cx
                .send_request(NewSessionRequest::new(work))
                .block_task()
                .await?;
            body(cx, session.session_id.0.to_string(), log).await;
            Ok(())
        });
    tokio::time::timeout(PATIENCE, run)
        .await
        .expect("the test timed out")
        .expect("the connection failed");
}

/// Send `text` and wait for the answer, logging it as `label`.
async fn prompt(
    cx: &ConnectionTo<Agent>,
    session: &str,
    log: &Log,
    label: &'static str,
    text: &str,
) -> StopReason {
    let response = cx
        .send_request(PromptRequest::new(
            session.to_owned(),
            vec![ContentBlock::from(text.to_owned())],
        ))
        .block_task()
        .await
        .expect("the prompt failed");
    log.push(Seen::Done(label));
    response.stop_reason
}

/// Wait until the client has seen tool call `id` start.
async fn until_tool(log: &Log, id: &str) {
    while !log.saw_tool_call(id) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_background_tool_task_wakes_the_session_after_the_prompt() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: bash, args: {command: "sleep 1; echo slept", background: true}}
  - text: "started."
  - text: "the task is done."
"#;
    with_session(script, Answers::default(), async |cx, session, log| {
        let stop = prompt(&cx, &session, &log, "go", "go").await;
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(log.transcript(), "started.|go|");
        // The task may run for good (a dev server), so the prompt does not
        // wait for it; the turn its completion starts still reaches the client.
        while !log.transcript().ends_with("done.") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(log.transcript(), "started.|go|the task is done.");
    })
    .await;
}

#[tokio::test]
async fn cancel_ends_the_prompt_as_cancelled() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: bash, args: {command: "sleep 30"}}
  - text: "never"
"#;
    with_session(script, Answers::default(), async |cx, session, log| {
        let running = prompt(&cx, &session, &log, "go", "go");
        let cancel = async {
            until_tool(&log, "c1").await;
            cx.send_notification(CancelNotification::new(session.clone()))
                .unwrap();
        };
        let (stop, ()) = tokio::join!(running, cancel);
        assert_eq!(stop, StopReason::Cancelled);
    })
    .await;
}

#[tokio::test]
async fn a_prompt_sent_while_busy_waits_its_turn() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: bash, args: {command: "sleep 1"}}
  - text: "first."
  - text: "second."
"#;
    with_session(script, Answers::default(), async |cx, session, log| {
        let first = prompt(&cx, &session, &log, "a", "one");
        let second = async {
            until_tool(&log, "c1").await;
            prompt(&cx, &session, &log, "b", "two").await
        };
        let (a, b) = tokio::join!(first, second);
        assert_eq!((a, b), (StopReason::EndTurn, StopReason::EndTurn));
        assert_eq!(log.transcript(), "first.|a|second.|b|");
    })
    .await;
}

#[tokio::test]
async fn steering_goes_into_the_running_turn() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: bash, args: {command: "sleep 1"}}
  - text: "saw it."
"#;
    with_session(script, Answers::default(), async |cx, session, log| {
        let idle = cx
            .send_request(SteerRequest {
                session_id: session.clone(),
                prompt: vec![ContentBlock::from("early".to_owned())],
                meta: Some(serde_json::json!({ "steering": { "idleBehavior": "promptRequired" } })),
            })
            .block_task()
            .await
            .unwrap();
        assert_eq!(idle.outcome, "promptRequired");
        let running = prompt(&cx, &session, &log, "go", "go");
        let steer = async {
            until_tool(&log, "c1").await;
            cx.send_request(SteerRequest {
                session_id: session.clone(),
                prompt: vec![ContentBlock::from("also this".to_owned())],
                meta: None,
            })
            .block_task()
            .await
            .unwrap()
        };
        let (stop, steered) = tokio::join!(running, steer);
        assert_eq!(steered.outcome, "injected");
        assert_eq!(stop, StopReason::EndTurn);
        // One turn took both: the script has no reply left for another.
        assert_eq!(log.transcript(), "saw it.|go|");
    })
    .await;
}

#[tokio::test]
async fn ask_without_elicitation_is_answered_not_left_waiting() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: ask, args: {questions: [{question: "Which color?"}]}}
  - text: "fine."
"#;
    with_session(script, Answers::default(), async |cx, session, log| {
        assert_eq!(
            prompt(&cx, &session, &log, "go", "go").await,
            StopReason::EndTurn
        );
        let (status, text) = log.tool_text("c1").unwrap();
        assert_eq!(status, ToolCallStatus::Completed);
        assert!(text.contains("No user can answer"), "{text}");
    })
    .await;
}

#[tokio::test]
async fn ask_with_elicitation_takes_the_form_answer() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: ask, args: {questions: [{question: "Which color?"}]}}
  - text: "fine."
"#;
    let answers = Answers {
        elicit: Some("blue"),
        ..Answers::default()
    };
    with_session(script, answers, async |cx, session, log| {
        assert_eq!(
            prompt(&cx, &session, &log, "go", "go").await,
            StopReason::EndTurn
        );
        let (_, text) = log.tool_text("c1").unwrap();
        assert!(text.contains("A: blue"), "{text}");
    })
    .await;
}

#[tokio::test]
async fn an_unknown_permission_answer_is_a_refusal() {
    let script = r#"
turns:
  - tool_calls:
      - {call_id: c1, name: card_mount, args: {at: project, create: elsewhere}}
  - text: "ok."
"#;
    let answers = Answers {
        permission: Some("something_else"),
        ..Answers::default()
    };
    with_session(script, answers, async |cx, session, log| {
        assert_eq!(
            prompt(&cx, &session, &log, "go", "go").await,
            StopReason::EndTurn
        );
        let (status, text) = log.tool_text("c1").unwrap();
        assert_eq!(status, ToolCallStatus::Failed);
        assert!(text.contains("declined"), "{text}");
    })
    .await;
}
