//! The user's approval of one assistant deck acting on another: mounting it,
//! writing to it, asking it or linking its cards. A grant (`deck.yaml`, or an
//! earlier "allow for this session" / "always allow") skips the question.
//! Nobody answering in time is no approval: the action is not taken.

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
    (action == Action::Write && state.project() == Some(to))
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
    let answer = ask_user(cx, request).await?;
    if answer.allow {
        decks.remember(action, answer.scope, &session, &state.assistant, to);
    }
    Ok(answer.allow)
}

/// Ask the user whatever the grants say, and remember nothing: for changes
/// to the grants themselves (a deck's `mounts` and `allow_ask`), which no
/// earlier answer may cover. The request is marked `once`, so the user is
/// offered only to allow it this once.
pub(crate) async fn confirm(cx: &Cx, mut request: Value) -> ToolResult<bool> {
    request["once"] = Value::Bool(true);
    Ok(ask_user(cx, request).await?.allow)
}

/// Put `request` to the user and wait for the answer.
async fn ask_user(cx: &Cx, request: Value) -> ToolResult<Answer> {
    let timeout = std::time::Duration::from_secs(crate::ask::ASK_TIMEOUT);
    let Some(answer) = cx
        .decide(request, Some(timeout))
        .await
        .map_err(|e| ToolError::model(e.to_string()))?
    else {
        // Told apart from a refusal: the user said nothing, not no.
        return Err(ToolError::model(format!(
            "the user did not answer within {} min, so this was not done; ask for it again once \
             they are back",
            crate::ask::ASK_TIMEOUT / 60
        )));
    };
    Ok(serde_json::from_value(answer).unwrap_or_default())
}

/// A deck's display name, or its id when it is gone.
pub(crate) fn deck_name(decks: &Decks, id: &str) -> String {
    decks
        .get(id)
        .map(|d| d.meta.name)
        .unwrap_or_else(|| id.to_owned())
}
