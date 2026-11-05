//! The user's approval of one assistant deck acting on another: mounting it,
//! writing to it, asking it or linking its cards. A grant (`deck.yaml`, or an
//! earlier "allow for this session" / "always allow") skips the question.

use rumu_cards::Decks;
use rumu_cards::deck::{Action, Scope};
use rumu_core::{Cx, ToolError, ToolResult};
use serde::Deserialize;
use serde_json::Value;

/// The user's answer to a `decision.requested`.
#[derive(Debug, Default, Deserialize)]
struct Answer {
    #[serde(default)]
    allow: bool,
    #[serde(default)]
    scope: Scope,
}

/// Is the session's assistant allowed to take `action` on the deck `to`?
/// Asks the user with `request` (the `decision.requested` payload) unless a
/// grant already covers it, and remembers the answer for as long as the user
/// chose. `Ok(false)` when the user declines.
pub(crate) async fn approve(
    decks: &Decks,
    cx: &Cx,
    action: Action,
    to: &str,
    request: Value,
) -> ToolResult<bool> {
    let from = cx.state()?.assistant.clone();
    let session = cx.session.to_string();
    if decks.allowed(action, &session, &from, to) {
        return Ok(true);
    }
    let answer = cx
        .decide(request)
        .await
        .map_err(|e| ToolError::model(e.to_string()))?;
    let answer: Answer = serde_json::from_value(answer).unwrap_or_default();
    if answer.allow {
        decks.remember(action, answer.scope, &session, &from, to);
    }
    Ok(answer.allow)
}

/// A deck's display name, or its id when it is gone.
pub(crate) fn deck_name(decks: &Decks, id: &str) -> String {
    decks
        .get(id)
        .map(|d| d.meta.name)
        .unwrap_or_else(|| id.to_owned())
}
