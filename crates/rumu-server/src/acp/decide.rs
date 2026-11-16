//! Questions rumu puts to the user (`decision.requested`), answered through
//! the client: approvals as `session/request_permission`, `ask` as an
//! `elicitation/create` form. Whatever the client cannot answer, or answers
//! in a way not understood, is declined — never left waiting.

use agent_client_protocol::Client;
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CreateElicitationRequest, ElicitationAction, ElicitationContentValue, PermissionOption,
    PermissionOptionKind, RequestPermissionOutcome, RequestPermissionRequest, ToolCallUpdate,
    ToolCallUpdateFields,
};
use serde_json::{Map, Value, json};

use super::Rumu;

/// The approval kinds rumu asks about (`rumu_plugins::approve`).
const APPROVALS: &[&str] = &[
    "card_mount",
    "card_write",
    "card_link",
    "ask",
    "project_create",
    "workspace_change",
];

/// Answer one `decision.requested` (`params`) of the ACP session `session`.
/// `elicit` says whether the client takes form elicitations.
pub(super) async fn answer(
    rumu: Rumu,
    cx: ConnectionTo<Client>,
    session: String,
    elicit: bool,
    params: Value,
) {
    let Some(id) = params.get("id").and_then(Value::as_str) else {
        return;
    };
    let call = params
        .get("call_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let request = params.get("request").cloned().unwrap_or(Value::Null);
    let kind = request
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let answer = match kind {
        "questions" if elicit => questions(&cx, &session, &call, &request).await,
        "questions" => no_user(&request),
        k if APPROVALS.contains(&k) => approval(&cx, &session, &call, &request).await,
        // A question this version does not know: declined.
        _ => json!({}),
    };
    // The decision may be gone already (its turn was aborted).
    let _ = rumu
        .call("decision.answer", json!({ "id": id, "answer": answer }))
        .await;
}

/// An approval as a permission request: allow once, always allow, or deny.
async fn approval(cx: &ConnectionTo<Client>, session: &str, call: &str, request: &Value) -> Value {
    let field = |key: &str| request.get(key).and_then(Value::as_str).unwrap_or_default();
    let title = match field("kind") {
        "card_mount" => format!("Mount deck `{}` at `{}`", field("deck"), field("at")),
        "card_write" => format!("Write card `{}` in deck `{}`", field("path"), field("deck")),
        "card_link" => format!("Link `{}` from deck `{}`", field("target"), field("to")),
        "ask" => format!("Hand a task to `{}`: {}", field("to"), field("task")),
        "project_create" => format!("Create project `{}`", field("name")),
        "workspace_change" => format!("Change workspace `{}`", field("name")),
        kind => kind.to_owned(),
    };
    let options = vec![
        PermissionOption::new("allow_once", "Allow", PermissionOptionKind::AllowOnce),
        PermissionOption::new(
            "allow_always",
            "Always allow",
            PermissionOptionKind::AllowAlways,
        ),
        PermissionOption::new("reject_once", "Deny", PermissionOptionKind::RejectOnce),
    ];
    let tool_call = ToolCallUpdate::new(
        call.to_owned(),
        ToolCallUpdateFields::new()
            .title(title)
            .raw_input(request.clone()),
    );
    let outcome = cx
        .send_request(RequestPermissionRequest::new(
            session.to_owned(),
            tool_call,
            options,
        ))
        .block_task()
        .await;
    match outcome.map(|r| r.outcome) {
        Ok(RequestPermissionOutcome::Selected(s)) => match &*s.option_id.0 {
            "allow_once" => json!({ "allow": true, "scope": "once" }),
            "allow_always" => json!({ "allow": true, "scope": "always" }),
            _ => json!({ "allow": false }),
        },
        _ => json!({ "allow": false }),
    }
}

/// The answer to `ask` when there is no one to ask: each question answered
/// with that, so the model goes on by itself instead of waiting.
fn no_user(request: &Value) -> Value {
    let n = request
        .get("questions")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let answer = json!({
        "custom": "No user can answer here (the client takes no questions); decide on your own."
    });
    json!({ "answers": vec![answer; n] })
}

/// `ask` as a form: per question a choice (single or several) when it has
/// options, plus a free-text field.
async fn questions(cx: &ConnectionTo<Client>, session: &str, call: &str, request: &Value) -> Value {
    let questions: Vec<Value> = request
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut properties = Map::new();
    let mut message = vec![];
    for (i, q) in questions.iter().enumerate() {
        let text = q
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let title = q.get("header").and_then(Value::as_str).unwrap_or(text);
        message.push(format!("{}. {text}", i + 1));
        let options: Vec<Value> = q
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|o| {
                let label = o.get("label").cloned().unwrap_or(Value::Null);
                let mut option = json!({ "const": label, "title": label });
                if let Some(d) = o.get("description") {
                    option["description"] = d.clone();
                }
                option
            })
            .collect();
        let key = format!("q{}", i + 1);
        if options.is_empty() {
            properties.insert(
                key,
                json!({ "type": "string", "title": title, "description": text }),
            );
            continue;
        }
        let pick = if q.get("multiple").and_then(Value::as_bool) == Some(true) {
            json!({ "type": "array", "title": title, "description": text, "items": { "anyOf": options } })
        } else {
            json!({ "type": "string", "title": title, "description": text, "oneOf": options })
        };
        properties.insert(key.clone(), pick);
        properties.insert(
            format!("{key}_own"),
            json!({ "type": "string", "title": "Or in your own words" }),
        );
    }
    let request = json!({
        "mode": "form",
        "sessionId": session,
        "toolCallId": call,
        "message": message.join("\n"),
        "requestedSchema": { "type": "object", "properties": properties },
    });
    let Ok(request) = serde_json::from_value::<CreateElicitationRequest>(request) else {
        return json!({});
    };
    let Ok(response) = cx.send_request(request).block_task().await else {
        return json!({});
    };
    let ElicitationAction::Accept(accept) = response.action else {
        return json!({});
    };
    let content = accept.content.unwrap_or_default();
    let text = |key: &str| match content.get(key) {
        Some(ElicitationContentValue::String(s)) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    };
    let answers: Vec<Value> = (1..=questions.len())
        .map(|i| {
            let key = format!("q{i}");
            let selected = match content.get(&key) {
                Some(ElicitationContentValue::StringArray(v)) => v.clone(),
                Some(ElicitationContentValue::String(s)) => vec![s.clone()],
                _ => vec![],
            };
            // A question without options has only its free text.
            let has_options = questions[i - 1]
                .get("options")
                .and_then(Value::as_array)
                .is_some_and(|o| !o.is_empty());
            let custom = if has_options {
                text(&format!("{key}_own"))
            } else {
                selected.first().cloned()
            };
            let selected = if has_options { selected } else { vec![] };
            json!({ "selected": selected, "custom": custom })
        })
        .collect();
    json!({ "answers": answers })
}
