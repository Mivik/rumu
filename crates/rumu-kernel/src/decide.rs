//! Questions put to the user on behalf of a tool call (`Cx::decide`), and
//! one-turn sessions run on behalf of one (`Cx::consult`).
//!
//! A decision is runtime-only: a `decision.requested` push the client answers
//! with `decision.answer` (by id), resolved with a `decision.resolved` push.
//! It dies with the turn: aborting the branch's loop cancels the wait, and a
//! restart loses it like any other in-flight call.

use std::sync::Arc;

use rumu_core::{BranchId, Consult, ConsultReply, Content, SessionRoot};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{Error, Kernel, Result};

/// A decision waiting for the user.
pub(crate) struct PendingDecision {
    /// The `decision.requested` params, replayed by `decision.list`.
    pub params: Value,
    answer: oneshot::Sender<Value>,
}

/// Retires a decision however its wait ends: answered, cancelled or dropped.
struct Resolved {
    kernel: Arc<Kernel>,
    id: String,
}

impl Drop for Resolved {
    fn drop(&mut self) {
        self.kernel.inner.lock().unwrap().decisions.remove(&self.id);
        self.kernel
            .push("decision.resolved", json!({ "id": self.id }));
    }
}

impl Kernel {
    /// The loop token of `branch`, or a token nothing cancels when no loop
    /// runs there.
    fn loop_token(&self, branch: &BranchId) -> CancellationToken {
        self.inner
            .lock()
            .unwrap()
            .running
            .get(branch)
            .cloned()
            .unwrap_or_default()
    }

    /// Put `request` to the user and wait for the answer, or for the
    /// branch's loop to be aborted.
    pub(crate) async fn decide(
        self: &Arc<Self>,
        branch: &BranchId,
        call_id: &str,
        request: Value,
    ) -> Result<Value> {
        let token = self.loop_token(branch);
        let session = self.session_of(branch)?;
        let (tx, rx) = oneshot::channel();
        let id = {
            let mut inner = self.inner.lock().unwrap();
            inner.decision_seq += 1;
            let id = format!("decision-{}", inner.decision_seq);
            let params = json!({
                "id": id,
                "session": session,
                "branch": branch,
                "call_id": call_id,
                "request": request,
            });
            inner.decisions.insert(
                id.clone(),
                PendingDecision {
                    params: params.clone(),
                    answer: tx,
                },
            );
            self.push("decision.requested", params);
            id
        };
        // An abort usually drops this future mid-wait (the kernel abandons the
        // tool call), so the cleanup lives in a guard, not after the await.
        let _resolved = Resolved {
            kernel: self.clone(),
            id,
        };
        tokio::select! {
            biased;
            _ = token.cancelled() => Err(Error::DecisionCancelled),
            answer = rx => answer.map_err(|_| Error::DecisionCancelled),
        }
    }

    /// `decision.answer`: hand the user's answer to the waiting call.
    pub fn answer_decision(&self, id: &str, answer: Value) -> Result<()> {
        let pending = self
            .inner
            .lock()
            .unwrap()
            .decisions
            .remove(id)
            .ok_or_else(|| Error::UnknownDecision(id.to_owned()))?;
        // The waiter may have just been cancelled; then the answer is moot.
        let _ = pending.answer.send(answer);
        Ok(())
    }

    /// `decision.list`: every decision still waiting, oldest first.
    pub fn decisions(&self) -> Vec<Value> {
        let inner = self.inner.lock().unwrap();
        let mut list: Vec<(u64, Value)> = inner
            .decisions
            .iter()
            .map(|(id, d)| {
                let seq = id.trim_start_matches("decision-").parse().unwrap_or(0);
                (seq, d.params.clone())
            })
            .collect();
        list.sort_by_key(|(seq, _)| *seq);
        list.into_iter().map(|(_, v)| v).collect()
    }

    /// Run `consult` as a new session of its assistant: one prompt, one loop. Its
    /// token is a child of `from`'s loop, so aborting the asking turn aborts
    /// the answer. The turn is spawned: an abort drops the asking tool call,
    /// and this future with it, while the turn still has to wind down.
    pub(crate) async fn consult(
        self: &Arc<Self>,
        from: &BranchId,
        consult: Consult,
    ) -> Result<ConsultReply> {
        let token = self.loop_token(from).child_token();
        let (info, branch) = self
            .create_rooted_session(SessionRoot {
                assistant: consult.assistant,
                asked_by: Some(consult.asked_by),
                mounts: consult.mounts,
                mcp: consult.mcp,
            })
            .await?;
        self.inner
            .lock()
            .unwrap()
            .running
            .insert(branch.id.clone(), token.clone());
        let (tx, rx) = oneshot::channel();
        let kernel = self.clone();
        tokio::spawn(async move {
            let run = kernel
                .consult_turn(&branch.id, consult.prompt, &token)
                .await;
            let (text, failure) = match run {
                Ok(text) => (Ok(text), None),
                Err(e) => (Err(e.to_string()), Some(e)),
            };
            kernel.finish_turn(branch.id, failure).await;
            let _ = tx.send(text);
        });
        let text = rx
            .await
            .map_err(|_| Error::Internal("consulted turn vanished".into()))?
            .map_err(|e| Error::Internal(format!("consulted session failed: {e}")))?;
        Ok(ConsultReply {
            session: info.id,
            text,
        })
    }

    /// The consulted session's turn: its final reply, or `None` when a hook
    /// blocked the prompt or the turn was aborted.
    async fn consult_turn(
        self: &Arc<Self>,
        branch: &BranchId,
        prompt: Content,
        token: &CancellationToken,
    ) -> Result<Option<String>> {
        if self.open_turn(branch, prompt, true, "consult").await? {
            return Ok(None);
        }
        let outcome = self.run_loop(branch.clone(), token).await?;
        self.push("turn.done", json!({ "branch": branch }));
        Ok((!outcome.stopped_early).then_some(outcome.last_text))
    }
}
