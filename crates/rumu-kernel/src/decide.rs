//! Questions put to the user on behalf of a tool call (`Cx::decide`), and
//! other assistants' sessions run on behalf of one (`Cx::delegate`).
//!
//! A decision is runtime-only: a `decision.requested` push the client answers
//! with `decision.answer` (by id), resolved with a `decision.resolved` push.
//! It dies with the turn: aborting the branch's loop cancels the wait, and a
//! restart loses it like any other in-flight call. One given a timeout also
//! ends unanswered when it runs out — its params carry the deadline
//! (`expires_at`) — unless the client holds it (`decision.hold`, once the
//! user starts on it), which stops the clock.

use std::sync::Arc;
use std::time::Duration;

use rumu_core::{BranchId, Content, Delegation, DelegationReply, SessionRoot};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{Error, Kernel, Result};

/// A decision waiting for the user.
pub(crate) struct PendingDecision {
    /// The `decision.requested` params, replayed by `decision.list`.
    pub params: Value,
    answer: oneshot::Sender<Value>,
    /// Stops the clock of a decision with a timeout; taken by a hold.
    hold: Option<oneshot::Sender<()>>,
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

    /// Put `request` to the user and wait for the answer, for `timeout` to
    /// run out unless held (`None`), or for the branch's loop to be aborted.
    pub(crate) async fn decide(
        self: &Arc<Self>,
        branch: &BranchId,
        call_id: &str,
        request: Value,
        timeout: Option<Duration>,
    ) -> Result<Option<Value>> {
        let token = self.loop_token(branch);
        let session = self.session_of(branch)?;
        let (tx, rx) = oneshot::channel();
        let (hold_tx, hold_rx) = oneshot::channel();
        let id = {
            let mut inner = self.inner.lock().unwrap();
            inner.decision_seq += 1;
            let id = format!("decision-{}", inner.decision_seq);
            let mut params = json!({
                "id": id,
                "session": session,
                "branch": branch,
                "call_id": call_id,
                "request": request,
            });
            if let Some(timeout) = timeout {
                let ms = i64::try_from(timeout.as_millis()).unwrap_or(i64::MAX);
                params["expires_at"] = json!(rumu_core::now().saturating_add(ms));
            }
            inner.decisions.insert(
                id.clone(),
                PendingDecision {
                    params: params.clone(),
                    answer: tx,
                    hold: timeout.map(|_| hold_tx),
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
        let expire = async {
            let Some(timeout) = timeout else {
                return std::future::pending().await;
            };
            tokio::select! {
                () = tokio::time::sleep(timeout) => {}
                // Held: the clock stops for good.
                _ = hold_rx => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            _ = token.cancelled() => Err(Error::DecisionCancelled),
            answer = rx => answer.map(Some).map_err(|_| Error::DecisionCancelled),
            () = expire => Ok(None),
        }
    }

    /// `decision.hold`: stop the clock of a decision the user has started on,
    /// so it waits for their answer. A no-op for one without a timeout.
    pub fn hold_decision(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.lock().unwrap();
        let pending = inner
            .decisions
            .get_mut(id)
            .ok_or_else(|| Error::UnknownDecision(id.to_owned()))?;
        if let Some(hold) = pending.hold.take() {
            let _ = hold.send(());
            if let Some(params) = pending.params.as_object_mut() {
                params.remove("expires_at");
            }
            drop(inner);
            self.push("decision.held", json!({ "id": id }));
        }
        Ok(())
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

    /// Run `delegation` as a new session of its assistant: one prompt, one
    /// loop. Its token is a child of `from`'s loop, so aborting the handing
    /// turn aborts it. The turn is spawned: an abort drops the handing tool
    /// call, and this future with it, while the turn still has to wind down.
    pub(crate) async fn delegate(
        self: &Arc<Self>,
        from: &BranchId,
        delegation: Delegation,
    ) -> Result<DelegationReply> {
        // Its own model unless the call asks for another; handed to yourself,
        // the model this path runs on.
        let parent = self.state_of_nodes(&self.pending_path_nodes(from)?);
        let own = parent.assistant == delegation.assistant;
        let base = if own {
            self.model_of(&parent)
        } else {
            let lookup = self.inner.lock().unwrap().assistant_model.clone();
            lookup(&delegation.assistant)
        };
        let model = self
            .model_override(delegation.model, delegation.effort, base.as_ref())
            .map_err(Error::ModelPick)?
            .or_else(|| parent.model.filter(|_| own));
        let token = self.loop_token(from).child_token();
        let (info, branch) = self
            .create_rooted_session(SessionRoot {
                assistant: delegation.assistant,
                delegated_by: Some(delegation.delegated_by),
                mounts: delegation.mounts,
            })
            .await?;
        if model.is_some() {
            self.use_model(&branch.id, model).await?;
        }
        self.inner
            .lock()
            .unwrap()
            .running
            .insert(branch.id.clone(), token.clone());
        let (tx, rx) = oneshot::channel();
        let kernel = self.clone();
        tokio::spawn(async move {
            let run = kernel
                .delegated_turn(&branch.id, delegation.prompt, &token)
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
            .map_err(|_| Error::Internal("delegated turn vanished".into()))?
            .map_err(|e| Error::Internal(format!("delegated session failed: {e}")))?;
        Ok(DelegationReply {
            session: info.id,
            text,
        })
    }

    /// The delegated session's turn: its final reply, or `None` when a hook
    /// blocked the prompt or the turn was aborted.
    async fn delegated_turn(
        self: &Arc<Self>,
        branch: &BranchId,
        prompt: Content,
        token: &CancellationToken,
    ) -> Result<Option<String>> {
        if self.open_turn(branch, prompt, true, "delegate").await? {
            return Ok(None);
        }
        let outcome = self.run_loop(branch.clone(), token).await?;
        self.push("turn.done", json!({ "branch": branch }));
        Ok((!outcome.stopped_early).then_some(outcome.last_text))
    }
}
