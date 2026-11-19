//! The actions on `@rumu:/sessions`: `session.create` starts a session of an
//! assistant (yourself by default) on a prompt, `session.wait` waits until a
//! session is idle and returns its final reply, and `branch.send` messages
//! the agent on a branch (a sub-agent, a session's main line). Together they
//! hand work to a session of its own; with `invoke`'s `background: true`,
//! the wait is a task whose completion note says the session is done.
//!
//! A started session is one like any other: it runs on by itself, nothing of
//! the session that started it stops it, and it may start sessions in turn.
//! Another assistant's session asks the user first (see
//! [`crate::approve`]); `allow_ask` in its `deck.yaml` grants it up front.

use rumu_cards::deck::Action;
use rumu_core::{
    BranchKind, CardMount, Content, Effort, NewSession, NodeBody, SessionId, ToolError, TurnError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::WorkspacePlugin;
use super::session::can_see;
use super::vfs::{At, Out};

/// `session.create` arguments.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateArgs {
    /// its first message, self-contained: it sees nothing of this conversation
    prompt: String,
    /// the id of the assistant deck that runs it; yourself when absent
    #[serde(default)]
    assistant: Option<String>,
    /// a model alias (`rumu/<name>`) to run on instead of its own model
    #[serde(default)]
    model: Option<String>,
    /// the reasoning effort to run at
    #[serde(default)]
    effort: Option<Effort>,
}

/// `branch.send` arguments.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SendArgs {
    /// the message, self-contained: say what you want back
    text: String,
}

impl WorkspacePlugin {
    /// `session.create`: a session of `assistant` (yourself by default; another
    /// one with the user's approval), with its own mounts and the project,
    /// prompted with `prompt`.
    pub(super) async fn create_session(&self, at: &At<'_>, args: CreateArgs) -> Out {
        let state = at.state;
        let from = &state.assistant;
        let prompt = args.prompt.trim();
        if prompt.is_empty() {
            return Err("session.create: `prompt` must not be empty".into());
        }
        let Some(from_deck) = self.decks.get(from) else {
            return Err(ToolError::model(format!(
                "session.create: this session's assistant deck `{from}` is gone"
            )));
        };
        let to_id = args.assistant.unwrap_or_else(|| from.clone());
        let own = &to_id == from;
        let Some(to) = self
            .decks
            .get(&to_id)
            .filter(|d| d.assistant() && self.decks.visible(d.id(), from, &state.mounts))
        else {
            let names: Vec<String> = self
                .decks
                .assistants()
                .into_iter()
                .filter(|d| d.id() != from && self.decks.visible(d.id(), from, &state.mounts))
                .map(|d| format!("`{}` ({})", d.id(), d.meta.name))
                .collect();
            return Err(ToolError::model(format!(
                "session.create: no assistant with id `{to_id}` (assistants: {})",
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            )));
        };
        let here = format!("{}:/sessions/{}", rumu_core::RUMU_WORKSPACE, at.cx.session);
        // Yourself needs no approval: a session of your own gets nothing a new
        // one would not.
        let text = if own {
            format!(
                "You sent this to yourself from session `{here}`:\n\n{prompt}\n\n\
                 That session may wait on this one for your final reply, so end with a concise \
                 summary."
            )
        } else {
            let request = json!({
                "kind": "ask",
                "from": from_deck.meta.name,
                "from_id": from,
                "to": to.meta.name,
                "to_id": to.id(),
                "task": prompt,
            });
            let allowed =
                crate::approve::approve(&self.decks, at.cx, Action::Ask, to.id(), request)
                    .await
                    .map_err(|e| ToolError::model(format!("session.create: {e}")))?;
            if !allowed {
                return Err(ToolError::model(format!(
                    "session.create: the user declined a session of `{}`",
                    to.meta.name
                )));
            }
            format!(
                "The assistant `{}` sends this from its session `{here}`, with the user's \
                 permission:\n\n{prompt}\n\n\
                 It may wait on this session for your final reply, so end with a concise summary.",
                from_deck.meta.name
            )
        };
        // Its own default mounts, and the project of this session.
        let mut mounts: Vec<CardMount> = self.decks.default_mounts(to.id());
        if let Some(project) = &state.project {
            mounts.push(CardMount {
                deck: project.clone(),
                at: rumu_core::PROJECT_MOUNT.into(),
            });
        }
        mounts.sort_by(|a, b| a.at.cmp(&b.at));
        let (session, _) = at
            .cx
            .start_session(NewSession {
                assistant: to.id().to_owned(),
                mounts,
                model: args.model,
                effort: args.effort,
                prompt: Content::text(text),
            })
            .await
            .map_err(|e| ToolError::model(format!("session.create: {e}")))?;
        self.started
            .lock()
            .unwrap()
            .insert((at.cx.session.clone(), session.clone()));
        let path = format!("{}:/sessions/{session}", rumu_core::RUMU_WORKSPACE);
        Ok(format!(
            "started session `{path}` of `{}`; it runs on by itself. `invoke` `session.wait` on \
             `{path}` for its final reply (with `background: true`, a note brings it when the \
             session is done).",
            to.meta.name
        )
        .into())
    }

    /// `branch.send`: message the agent on a branch of a session this path may
    /// see (or one it started). Not your own branch, nor a side question
    /// (it has no tools to act on it). Another assistant's session asks the
    /// user first, as `session.create` does. The message says who sends it:
    /// a sub-agent's parent needs no address (the reply comes back by
    /// itself), anyone else gives theirs to answer on.
    pub(super) async fn send_branch(&self, at: &At<'_>, args: SendArgs) -> Out {
        let text = args.text.trim();
        if text.is_empty() {
            return Err("branch.send: `text` must not be empty".into());
        }
        let (sid, bid) = (at.param("sid"), at.param("bid"));
        let sid = SessionId::from(sid);
        let missing = || ToolError::model(format!("no session `{sid}`"));
        let (info, _, branches) = at.cx.history().session_tree(&sid).map_err(|_| missing())?;
        let started = self
            .started
            .lock()
            .unwrap()
            .contains(&(at.cx.session.clone(), sid.clone()));
        if !can_see(at.state, &info) && !started {
            return Err(missing());
        }
        let target = super::session::find_branch(&branches, bid, &sid.0)?;
        let to = rumu_core::branch_address(&sid, &target.id);
        if target.id == at.cx.branch {
            return Err("branch.send: this is your own branch".into());
        }
        if matches!(target.kind, BranchKind::Side { .. }) {
            return Err("branch.send: a side question cannot act on messages".into());
        }
        let from = &at.state.assistant;
        let from_name = self
            .decks
            .get(from)
            .map_or_else(|| from.clone(), |d| d.meta.name.clone());
        if &info.assistant != from {
            let to_name = self
                .decks
                .get(&info.assistant)
                .map_or_else(|| info.assistant.clone(), |d| d.meta.name.clone());
            let request = json!({
                "kind": "ask",
                "from": from_name,
                "from_id": from,
                "to": to_name,
                "to_id": info.assistant,
                "task": text,
            });
            let allowed =
                crate::approve::approve(&self.decks, at.cx, Action::Ask, &info.assistant, request)
                    .await
                    .map_err(|e| ToolError::model(format!("branch.send: {e}")))?;
            if !allowed {
                return Err(ToolError::model(format!(
                    "branch.send: the user declined a message to `{to_name}`"
                )));
            }
        }
        let parent = matches!(&target.kind,
            BranchKind::Agent { parent_branch, .. } if *parent_branch == at.cx.branch);
        let (message, back) = if parent {
            (
                format!("A message from your parent:\n\n{text}"),
                "its reply comes back to you as a note",
            )
        } else {
            let here = rumu_core::branch_address(&at.cx.session, &at.cx.branch);
            (
                format!(
                    "A message from `{from_name}` at `{here}` — `invoke` `branch.send` there \
                     to answer:\n\n{text}"
                ),
                "it may answer with `branch.send`",
            )
        };
        at.cx
            .send(&target.id, Content::text(message))
            .await
            .map_err(|e| ToolError::model(format!("branch.send: {e}")))?;
        Ok(format!("sent to `{to}`; {back}").into())
    }

    /// `session.wait`: wait until the session's main line is idle, then
    /// return its final reply. A session this one started may be waited on
    /// even when its transcript is not visible here (another assistant's,
    /// outside any project): its reply was asked for.
    pub(super) async fn wait_session(&self, at: &At<'_>) -> Out {
        let sid = SessionId::from(at.param("sid"));
        if sid == at.cx.session {
            return Err("session.wait: this is the session you are in".into());
        }
        let missing = || ToolError::model(format!("no session `{sid}`"));
        let (info, _, _) = at.cx.history().session_tree(&sid).map_err(|_| missing())?;
        let visible = can_see(at.state, &info);
        let started = self
            .started
            .lock()
            .unwrap()
            .contains(&(at.cx.session.clone(), sid.clone()));
        if !visible && !started {
            return Err(missing());
        }
        let branch = info.main_branch;
        at.cx
            .wait_idle(&branch)
            .await
            .map_err(|e| ToolError::model(format!("session.wait: {e}")))?;
        let (info, nodes, branches) = at.cx.history().session_tree(&sid).map_err(|_| missing())?;
        let name = self
            .decks
            .get(&info.assistant)
            .map_or_else(|| info.assistant.clone(), |d| d.meta.name.clone());
        // Walk up from the head to the last reply since the last prompt.
        let mut reply = None;
        let mut cur = branches
            .iter()
            .find(|b| b.id == branch)
            .map(|b| b.head.clone());
        while let Some(id) = cur {
            let Some(node) = nodes.iter().find(|n| n.id == id) else {
                break;
            };
            match &node.body {
                NodeBody::Assistant { content, .. } if !content.text.trim().is_empty() => {
                    reply = Some(content.text.clone());
                    break;
                }
                NodeBody::Note { content, .. } => {
                    if let Some(e) = content.decode::<TurnError>() {
                        reply = Some(format!("(its turn failed: {})", e.message));
                        break;
                    }
                }
                NodeBody::User { .. } => break,
                _ => {}
            }
            cur = node.parent.clone();
        }
        let reply = reply.unwrap_or_else(|| "(it stopped without a reply)".into());
        let mut text = format!("session `{sid}` of `{name}` is idle; its final reply:\n\n{reply}");
        if visible {
            text.push_str(&format!(
                "\n\n[transcript: {}:/sessions/{sid}]",
                rumu_core::RUMU_WORKSPACE
            ));
        }
        Ok(text.into())
    }
}
