//! The user's approval of one assistant deck acting on another: mounting it,
//! writing to it, asking it or linking its cards. A grant (`deck.yaml`, or an
//! earlier "allow for this session" / "always allow") skips the question.

use rumu_cards::Decks;
use rumu_cards::deck::{Action, Scope};
use rumu_core::{Cx, PathState, ToolError, ToolResult};
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

/// May `action` be taken on deck `to` without asking the user? An assistant
/// may do anything with its own deck; the session's project deck is likewise
/// its own for writes.
pub(crate) fn allowed(
    decks: &Decks,
    state: &PathState,
    session: &str,
    action: Action,
    to: &str,
) -> bool {
    (action == Action::Write && state.project.as_deref() == Some(to))
        || decks.allowed(action, session, &state.assistant, to)
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
    let state = cx.state()?;
    let session = cx.session.to_string();
    if allowed(decks, &state, &session, action, to) {
        return Ok(true);
    }
    let answer = cx
        .decide(request)
        .await
        .map_err(|e| ToolError::model(e.to_string()))?;
    let answer: Answer = serde_json::from_value(answer).unwrap_or_default();
    if answer.allow {
        decks.remember(action, answer.scope, &session, &state.assistant, to);
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
