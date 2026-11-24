//! The agent loop and the built-in `agent` tool.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::task::CancelSource;
use crate::{Error, Result};
use rumu_core::kind;
use rumu_core::{
    Branch, BranchId, BranchKind, BranchStatus, CODE_DENIED_TOOLS, Call, CallStats, CompactReason,
    Content, Cx, Effort, FILE_PATCH_TAG, Inherit, InheritContext, InnerCall, Invoke, ModelPick,
    ModelUse, NodeBody, NodeId, RUMU_WORKSPACE, SYSTEM_SOURCE, SessionId, Tagged, ToolCall,
    ToolError, ToolOutput, TurnError, WorkspaceSet, WorkspaceTarget, WorkspaceUse, parse_args,
    truncate_output,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::task::{Foreground, Handoff, ToolWork, foreground_limit};
use crate::{Choice, Delta, Kernel, MAX_INVOKE_DEPTH, MAX_LLM_RETRIES, host::KernelRuntime};

/// Backoff before the first retry; doubles from there.
const RETRY_BACKOFF_START: Duration = Duration::from_secs(1);
/// Backoff ceiling, however many retries are left.
const RETRY_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// The wait before retry `attempt` (0-based): exponential from
/// [`RETRY_BACKOFF_START`], capped, with jitter so clients that failed
/// together do not retry in lockstep.
fn retry_backoff(attempt: u32) -> Duration {
    let exp = RETRY_BACKOFF_START.saturating_mul(1 << attempt.min(5));
    let base = exp.min(RETRY_BACKOFF_CAP);
    // Wall-clock jitter; statistical quality is irrelevant here.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let scale = 60 + nanos % 81; // 60..=140 %
    base.saturating_mul(scale) / 100
}

/// Who issued a tool call: the model directly, or a plugin on the model's
/// behalf (which must pass through the same pre/post hooks). `name` is the
/// invoking plugin's manifest name (e.g. `"code"`).
#[derive(Clone, Copy)]
pub(crate) enum Caller<'a> {
    Model,
    Plugin { name: &'a str, parent_call: &'a str },
}

/// What `exec_tool_call` produced, before the caller records the result.
pub(crate) struct ExecutedTool {
    pub out: ToolOutput,
    /// The `tool.pre` payload after rewrite (name/args actually used); the
    /// caller adds `output`/`is_error` before `tool.post`.
    pub payload: Value,
    pub dur_ms: u64,
    /// The block reason when `tool.pre` blocked the call.
    pub blocked: Option<String>,
    /// The workspace a file tool or `bash` call addressed (the final
    /// `tool.pre`'s `Fields::workspace`).
    pub workspace: Option<String>,
}

/// The payload a result keeps on its node: the tool's own, else the
/// workspace the call addressed, for the UI to name.
fn with_target(data: Option<Tagged>, workspace: Option<String>) -> Option<Tagged> {
    data.or_else(|| workspace.map(|ws| Tagged::new(&WorkspaceTarget { ws })))
}

#[derive(Debug, Default)]
pub(crate) struct LoopOutcome {
    /// Text of the last assistant reply without tool calls (the exploration
    /// summary when the loop ended with one).
    pub last_text: String,
    /// True when the loop ended without a final reply (abort).
    pub stopped_early: bool,
}

/// What a background turn runs: a new prompt, a replay of an existing one, or
/// an in-place retry of a failed turn.
pub(crate) enum TurnKind {
    Prompt(Content),
    Regenerate(Content),
    Retry,
    /// A manual compaction, with what the summary should focus on.
    Compact(Option<String>),
}

impl Kernel {
    /// `prompt` WS method: validate, register the loop guard, spawn the turn.
    /// Progress is reported via pushes. A prompt on a busy branch is steered:
    /// its `prompt` event is dispatched now and the user node is queued for
    /// the running turn's next safe point.
    pub async fn prompt(
        self: &Arc<Self>,
        branch: &BranchId,
        content: impl Into<Content>,
    ) -> Result<()> {
        self.store
            .get_branch(branch)?
            .ok_or_else(|| Error::BranchNotFound(branch.clone()))?;
        self.reset_storm(branch);
        let token = {
            let mut inner = self.inner.lock().unwrap();
            if inner.running.contains_key(branch) {
                None
            } else {
                let token = CancellationToken::new();
                inner.running.insert(branch.clone(), token.clone());
                Some(token)
            }
        };
        match token {
            Some(token) => {
                self.refresh_status_of(branch, false);
                self.spawn_turn(branch.clone(), token, TurnKind::Prompt(content.into()))
            }
            None => self.steer_prompt(branch, content.into(), true).await?,
        }
        Ok(())
    }

    /// `branch.send`: a message from another agent to `branch` — into its
    /// running turn, or starting one. The same path as a steering prompt,
    /// but it counts toward the storm guard and never resets it, so agents
    /// messaging each other back and forth stop at the guard; only the user's
    /// own prompt starts the count over.
    pub async fn send(self: &Arc<Self>, branch: &BranchId, content: Content) -> Result<()> {
        self.store
            .get_branch(branch)?
            .ok_or_else(|| Error::BranchNotFound(branch.clone()))?;
        self.steer_prompt(branch, content, false).await
    }

    /// A user prompt while a turn is running: dispatch the (still
    /// interceptable) prompt event, then queue the user node as a wake item so
    /// the running turn sees it at its next safe point — or a new turn starts
    /// if that turn ends first. A stop hands the user's own prompt
    /// (`from_user`) back; another agent's message is written all the same.
    async fn steer_prompt(
        self: &Arc<Self>,
        branch: &BranchId,
        content: Content,
        from_user: bool,
    ) -> Result<()> {
        let session = self.session_of(branch)?;
        let mut ev = self.make_event(
            kind::PROMPT,
            session,
            branch.clone(),
            prompt_payload(&content),
        )?;
        if let Some(reason) = self.dispatch(&mut ev, true).await? {
            // The running turn keeps going, so this must not look like a
            // turn end: a plain prompt.blocked toast.
            self.push(
                "prompt.blocked",
                json!({ "branch": branch, "message": reason }),
            );
            return Ok(());
        }
        let content = rewritten(&ev, content);
        // The node's id is fixed now: `prompt.cancel` takes it back by id
        // until the running turn's next safe point writes it.
        let node = NodeId::new();
        self.push(
            "prompt.queued",
            json!({ "branch": branch, "node": node, "text": content.text }),
        );
        self.deliver_item(
            branch,
            node,
            NodeBody::User { content },
            Some((SYSTEM_SOURCE, true)),
            from_user,
        )
        .await?;
        // If the turn ended between the check and the delivery, the debounced
        // wake path starts a new turn; while busy it is a no-op.
        self.request_wake(branch, Some(SYSTEM_SOURCE), true, true);
        Ok(())
    }

    /// `prompt.cancel` WS method: take back a steering prompt still queued on
    /// `branch`, returning its content; [`Error::NotQueued`] once it has been
    /// written. The wake it asked for goes with it unless another queued
    /// item still wants one, so the running turn's end starts no turn for it.
    pub fn cancel_queued(&self, branch: &BranchId, node: &NodeId) -> Result<Content> {
        let mut inner = self.inner.lock().unwrap();
        let queue = inner.queue.get_mut(branch).ok_or(Error::NotQueued)?;
        let i = queue
            .iter()
            .position(|q| q.id == *node && matches!(q.body, NodeBody::User { .. }))
            .ok_or(Error::NotQueued)?;
        let NodeBody::User { content } = queue.remove(i).body else {
            unreachable!("matched a user body above");
        };
        let wakes = queue.iter().any(|q| q.wake);
        let user = queue
            .iter()
            .any(|q| q.wake && matches!(q.body, NodeBody::User { .. }));
        if queue.is_empty() {
            inner.queue.remove(branch);
        }
        if !wakes {
            inner.wake_pending.remove(branch);
        } else if let Some(pending) = inner.wake_pending.get_mut(branch) {
            pending.user = user;
        }
        Ok(content)
    }

    fn reset_storm(&self, branch: &BranchId) {
        self.inner.lock().unwrap().storm.remove(branch);
    }

    /// `branch.regenerate` WS method: move the session's main line onto the
    /// user message `node` and run the step loop again from there. The
    /// prompt event is replayed so hooks and memory injection behave as they
    /// did the first time, but the message itself is not appended again.
    /// `choice` is applied first, so the reply runs on the composer's current
    /// workspaces and model. `stop` is [`Kernel::move_head`]'s. Progress
    /// arrives via pushes; the main line is returned immediately.
    pub async fn regenerate(
        self: &Arc<Self>,
        node: &NodeId,
        choice: Choice,
        stop: bool,
    ) -> Result<Branch> {
        let n = self
            .store
            .get_node(node)?
            .ok_or_else(|| Error::NodeNotFound(node.clone()))?;
        let content = match &n.body {
            NodeBody::User { content } => content.clone(),
            _ => return Err(Error::NotAUserMessage),
        };
        let session = self
            .store
            .get_session(&n.session)?
            .ok_or_else(|| Error::SessionNotFound(n.session.clone()))?;
        let branch = self.move_head(&session.main_branch, &n.id, stop).await?;
        self.apply_choice(&branch.id, choice).await?;
        let token = self.begin_loop(&branch.id)?;
        self.spawn_turn(branch.id.clone(), token, TurnKind::Regenerate(content));
        Ok(branch)
    }

    /// `branch.retry` WS method: run the step loop again on a branch whose
    /// reply failed — in place, so no prompt event and no new user node; a
    /// `turn.error` marker left at the head is simply stepped over. Only a
    /// recorded failure qualifies: the marker a failed turn left behind an
    /// unanswered prompt — one whose last non-note node is a user message or
    /// a tool result (notes, hook imports with text included, never answer
    /// one). A prompt-hook refusal records nothing, so it cannot be replayed
    /// past the hook. The branch must not be running. `choice` is recorded
    /// first (see [`Kernel::apply_choice`]), so a failure fixed by picking
    /// another model retries on that model. Progress arrives via pushes.
    pub async fn retry(self: &Arc<Self>, branch: &BranchId, choice: Choice) -> Result<()> {
        self.store
            .get_branch(branch)?
            .ok_or_else(|| Error::BranchNotFound(branch.clone()))?;
        if self.inner.lock().unwrap().running.contains_key(branch) {
            return Err(Error::TurnBusy);
        }
        self.reset_storm(branch);
        let mut recorded_failure = false;
        let owes = self
            .path_nodes(branch)?
            .iter()
            .rev()
            .find(|n| match &n.body {
                NodeBody::Note { content, .. } => {
                    recorded_failure |= content.decode::<TurnError>().is_some();
                    false
                }
                _ => true,
            })
            .is_some_and(|n| matches!(n.body, NodeBody::User { .. } | NodeBody::ToolResult { .. }));
        if !owes || !recorded_failure {
            return Err(Error::NothingToRetry);
        }
        self.apply_choice(branch, choice).await?;
        let token = self.begin_loop(branch)?;
        self.spawn_turn(branch.clone(), token, TurnKind::Retry);
        Ok(())
    }

    /// `branch.compact` WS method: compact an idle branch now, the summary
    /// focusing on `focus` when given. Progress arrives via pushes.
    pub async fn compact(self: &Arc<Self>, branch: &BranchId, focus: Option<String>) -> Result<()> {
        self.store
            .get_branch(branch)?
            .ok_or_else(|| Error::BranchNotFound(branch.clone()))?;
        let token = self.begin_loop(branch)?;
        self.spawn_turn(branch.clone(), token, TurnKind::Compact(focus));
        Ok(())
    }

    /// `branch.stop` WS method: stop the reply on `branch` and nothing else.
    /// The user's steering prompts the turn has not written yet come back,
    /// oldest first, for the composer (as `prompt.cancel` would); other
    /// queued items are still written but no longer start a turn, and posts
    /// arriving after the stop wake normally. Background tasks the branch
    /// started keep running — see [`Kernel::abort`] for the blunter version.
    pub fn stop(&self, branch: &BranchId) -> Result<Vec<(NodeId, Content)>> {
        let mut inner = self.inner.lock().unwrap();
        let mut back = Vec::new();
        if let Some(queue) = inner.queue.get_mut(branch) {
            queue.retain_mut(|q| match &mut q.body {
                NodeBody::User { content } if q.prompt => {
                    back.push((q.id.clone(), std::mem::take(content)));
                    false
                }
                _ => true,
            });
            if queue.is_empty() {
                inner.queue.remove(branch);
            }
        }
        Self::halt_locked(&mut inner, branch);
        drop(inner);
        self.signal_tasks();
        Ok(back)
    }

    /// Cancel the loop on `branch` and the wake it was owed.
    fn halt_locked(inner: &mut crate::Inner, branch: &BranchId) {
        if let Some(t) = inner.running.get(branch) {
            t.cancel();
        }
        inner.wake_pending.remove(branch);
    }

    /// `abort` WS method: stop the branch and cancel every background task it
    /// (or a sub-agent below it) started. Their completion notes are still
    /// written, without waking, and so is every queued item.
    pub fn abort(&self, branch: &BranchId) -> Result<()> {
        self.abort_tasks(branch, true, CancelSource::User);
        Self::halt_locked(&mut self.inner.lock().unwrap(), branch);
        self.signal_tasks();
        Ok(())
    }

    /// Register the loop guard for `branch`, returning its cancellation token.
    fn begin_loop(&self, branch: &BranchId) -> Result<CancellationToken> {
        let mut inner = self.inner.lock().unwrap();
        if inner.running.contains_key(branch) {
            return Err(Error::TurnBusy);
        }
        let t = CancellationToken::new();
        inner.running.insert(branch.clone(), t.clone());
        drop(inner);
        self.refresh_status_of(branch, false);
        Ok(t)
    }

    /// Run a turn in the background and release its loop guard when it ends.
    fn spawn_turn(self: &Arc<Self>, branch: BranchId, token: CancellationToken, turn: TurnKind) {
        let k = self.clone();
        tokio::spawn(async move {
            let run = {
                let (k, branch, token) = (k.clone(), branch.clone(), token.clone());
                async move { k.run_turn(branch, turn, token).await }
            };
            k.run_and_finish(branch, token, run).await;
        });
    }

    /// One user turn: prompt event → (unless regenerating) User node → the
    /// step loop. A regeneration moves onto an existing prompt, so its prompt
    /// event is replayed but the message is not appended again; a retry has
    /// neither — the loop simply runs again on what is already there. The
    /// loop's outcome comes back when it ran (not for a blocked prompt or a
    /// compaction).
    pub(crate) async fn run_turn(
        self: &Arc<Self>,
        branch: BranchId,
        turn: TurnKind,
        token: CancellationToken,
    ) -> Result<Option<LoopOutcome>> {
        match turn {
            TurnKind::Prompt(content) => {
                if self.open_turn(&branch, content, true, "prompt").await? {
                    return Ok(None);
                }
            }
            TurnKind::Regenerate(content) => {
                if self
                    .open_turn(&branch, content, false, "regenerate")
                    .await?
                {
                    return Ok(None);
                }
            }
            TurnKind::Retry => {
                self.push(
                    "turn.started",
                    json!({ "branch": branch, "reason": "retry" }),
                );
            }
            TurnKind::Compact(focus) => {
                self.push(
                    "turn.started",
                    json!({ "branch": branch, "reason": "compact" }),
                );
                self.flush_queue(&branch).await?;
                self.compact_now(&branch, CompactReason::Manual, focus.as_deref(), &token)
                    .await?;
                self.push("turn.done", json!({ "branch": branch }));
                return Ok(None);
            }
        }
        let outcome = self.run_loop(branch.clone(), &token).await?;
        self.push("turn.done", json!({ "branch": branch }));
        Ok(Some(outcome))
    }

    /// Announce a turn and dispatch its prompt event. Returns true when a hook
    /// blocked the prompt (turn.error has been pushed). `append_user` is false
    /// for a regeneration, whose prompt node is already on the branch.
    pub(crate) async fn open_turn(
        self: &Arc<Self>,
        branch: &BranchId,
        content: Content,
        append_user: bool,
        reason: &str,
    ) -> Result<bool> {
        self.push(
            "turn.started",
            json!({ "branch": branch, "reason": reason }),
        );
        let session = self.session_of(branch)?;

        // prompt event: interceptable (block / rewrite). Only `text` is
        // rewritable; the attachments ride along unchanged.
        let mut ev = self.make_event(
            kind::PROMPT,
            session,
            branch.clone(),
            prompt_payload(&content),
        )?;
        if let Some(reason) = self.dispatch(&mut ev, true).await? {
            // A refusal ends the turn (turn.error) but records nothing —
            // `blocked` marks it as such for the UI.
            self.push(
                "turn.error",
                json!({ "branch": branch, "message": reason, "blocked": true }),
            );
            return Ok(true);
        }
        if append_user {
            let content = rewritten(&ev, content);
            self.append_body(branch, NodeBody::User { content }, None)
                .await?;
        }
        Ok(false)
    }

    /// The step loop shared by turns and agent branches. It ends when the
    /// model replies without tool calls, when the turn is cancelled, or when a
    /// step fails.
    pub(crate) async fn run_loop(
        self: &Arc<Self>,
        branch: BranchId,
        token: &CancellationToken,
    ) -> Result<LoopOutcome> {
        let mut outcome = LoopOutcome::default();
        // A reply without tool calls is the only way a loop "finishes"; abort
        // leaves it without a final reply.
        let mut replied = false;
        // This reply's request overflowed once already.
        let mut overflowed = false;
        // A side question has no tools: its calls are refused once, and a
        // second batch ends the loop rather than going round again.
        let side = matches!(
            self.store.get_branch(&branch)?.map(|b| b.kind),
            Some(BranchKind::Side { .. })
        );
        let mut refused = false;

        loop {
            if token.is_cancelled() {
                break;
            }
            let session = self.session_of(&branch)?;

            // Flush first so llm.pre hooks see notes queued earlier on the path
            // (e.g. prompt-hook imports); compact when the view has outgrown
            // the window, so llm.pre sees (and re-injects into) the new view;
            // then llm.pre (hooks may inject), then flush again so those notes
            // are on the path before projection.
            self.flush_queue(&branch).await?;
            self.auto_compact(&branch, token).await?;
            let mut ev =
                self.make_event(kind::LLM_PRE, session.clone(), branch.clone(), json!({}))?;
            self.dispatch(&mut ev, false).await?;
            self.flush_queue(&branch).await?;
            // The request below projects everything just flushed; wake items
            // it carries have now been seen and must not start another turn.
            self.mark_wake_seen(&branch);

            // LLM call over the projection, replaying transient failures. A
            // failed attempt is retried only while nothing has streamed to
            // the UI yet: once text or reasoning reached the client,
            // replaying would duplicate it, so the turn fails instead.
            let path = self.path_nodes(&branch)?;
            let model = self.model_of(&crate::state::derive_state(&path));
            let pick = crate::compact::pick(&model);
            let nodes = rumu_core::view(&path);
            let registry = self.registry();
            let streamed = Arc::new(AtomicBool::new(false));
            let on_delta = {
                let k = self.clone();
                let dbranch = branch.clone();
                let streamed = streamed.clone();
                Some(Arc::new(move |d: Delta| {
                    streamed.store(true, Ordering::Relaxed);
                    // Pushed under the lock so `live_turns` snapshots exactly
                    // the deltas already broadcast.
                    let mut inner = k.inner.lock().unwrap();
                    let buf = inner.streaming.entry(dbranch.clone()).or_default();
                    buf.seq += 1;
                    let seq = buf.seq;
                    // `at`: where the chunk starts in its stream, so a
                    // client can drop the part a snapshot already holds
                    // when the ws feed has merged several deltas into one.
                    match d {
                        Delta::Text(t) => {
                            let at = buf.text_units;
                            buf.text_units += t.encode_utf16().count();
                            buf.text.push_str(&t);
                            k.push(
                                "llm.delta",
                                json!({ "branch": dbranch, "text": t, "seq": seq, "at": at }),
                            )
                        }
                        Delta::Reasoning(r) => {
                            let at = buf.reasoning_units;
                            buf.reasoning_units += r.encode_utf16().count();
                            buf.reasoning.push_str(&r);
                            k.push(
                                "llm.delta",
                                json!({ "branch": dbranch, "reasoning": r, "seq": seq, "at": at }),
                            )
                        }
                    }
                }) as crate::OnDelta)
            };
            // Restarted per attempt: the node's duration is the call that
            // produced it, not the backoff waits before it.
            let mut llm_started = Instant::now();
            let mut retries = 0u32;
            // `None` = cancelled (mid-call or in a backoff wait): nothing has
            // been appended, so the tree stays valid and the loop just ends.
            let resp = loop {
                tokio::select! {
                    // Abort wins even if the call is also ready.
                    biased;
                    _ = token.cancelled() => break None,
                    r = self.llm.complete(pick, &registry.system, &registry.tools, &nodes, on_delta.clone()) => {
                        match r {
                            Ok(resp) => break Some(Ok(resp)),
                            // Too long for the window: compact and project
                            // again, once per reply.
                            Err(e) if e.is_overflow()
                                && !overflowed
                                && !streamed.load(Ordering::Relaxed) => break Some(Err(e)),
                            Err(e) => {
                                if retries >= MAX_LLM_RETRIES
                                    || streamed.load(Ordering::Relaxed)
                                    || !e.is_transient()
                                {
                                    return Err(e.into());
                                }
                                tracing::warn!(
                                    %branch, attempt = retries + 1, error = %e,
                                    "transient LLM failure; retrying"
                                );
                                let backoff = retry_backoff(retries);
                                retries += 1;
                                self.push("turn.retrying", json!({
                                    "branch": branch,
                                    "attempt": retries,
                                    "max": MAX_LLM_RETRIES,
                                    "retry_in_ms": backoff.as_millis() as u64,
                                    "message": format!("{e}"),
                                }));
                                tokio::select! {
                                    _ = token.cancelled() => break None,
                                    _ = tokio::time::sleep(backoff) => {}
                                }
                                llm_started = Instant::now();
                            }
                        }
                    }
                }
            };
            let resp = match resp {
                None => break,
                Some(Ok(resp)) => resp,
                Some(Err(e)) => {
                    overflowed = true;
                    if self
                        .compact_now(&branch, CompactReason::Overflow, None, token)
                        .await?
                    {
                        continue;
                    }
                    return Err(e.into());
                }
            };
            overflowed = false;
            // The Assistant node below replaces the streamed text.
            self.inner.lock().unwrap().streaming.remove(&branch);
            let llm_ms = llm_started.elapsed().as_millis() as u64;

            self.append_body_dur(
                &branch,
                NodeBody::Assistant {
                    content: Content::text(resp.text.clone()),
                    reasoning: resp.reasoning.clone(),
                    stats: CallStats {
                        retries,
                        ..resp.stats.clone()
                    },
                    reasoning_reset: resp.reasoning_reset,
                    tool_calls: resp.tool_calls.clone(),
                    model: resp.model.clone(),
                    usage: resp.usage.clone(),
                },
                None,
                Some(llm_ms),
            )
            .await?;
            {
                let payload = json!({
                    "text": resp.text,
                    "tool_calls": resp.tool_calls,
                    "usage": resp.usage,
                });
                let mut ev =
                    self.make_event(kind::LLM_DONE, session.clone(), branch.clone(), payload)?;
                self.dispatch(&mut ev, false).await?;
            }

            // Track the latest assistant text even when it rode along with
            // tool calls: if the loop is stopped by an abort it still becomes
            // part of the exploration summary.
            if !resp.text.is_empty() {
                outcome.last_text = resp.text.clone();
            }
            if resp.tool_calls.is_empty() {
                replied = true;
                break;
            }
            if side {
                self.fail_open_calls(&branch, &resp.tool_calls, crate::side::SIDE_NO_TOOLS)
                    .await?;
                if refused {
                    break;
                }
                refused = true;
                continue;
            }

            // Tool batch (sequential is fine).
            let calls = resp.tool_calls.clone();
            let mut aborted = false;
            for call in &calls {
                let step = if token.is_cancelled() {
                    None
                } else {
                    self.exec_tool_call(&branch, &session, call, token, Caller::Model, 0)
                        .await?
                };
                let Some(ExecutedTool {
                    out,
                    mut payload,
                    dur_ms,
                    workspace,
                    ..
                }) = step
                else {
                    // Cancelled: unanswered calls get error results so the
                    // tree stays valid.
                    self.fail_open_calls(&branch, &calls, "aborted").await?;
                    aborted = true;
                    break;
                };
                payload["output"] = json!(out.content.text);
                payload["is_error"] = json!(out.is_error);
                let is_error = out.is_error;
                // The whole content, attachments included, becomes the node.
                let mut content = out.content.clone();
                content.data = with_target(content.data, workspace);
                self.append_body_dur(
                    &branch,
                    NodeBody::ToolResult {
                        call_id: call.call_id.clone(),
                        content,
                        is_error,
                        invoke: None,
                    },
                    None,
                    Some(dur_ms),
                )
                .await?;
                let mut ev =
                    self.make_event(kind::TOOL_POST, session.clone(), branch.clone(), payload)?;
                self.dispatch(&mut ev, false).await?;
            }
            if aborted {
                break;
            }

            // Safe point: flush queued notes/markers.
            self.flush_queue(&branch).await?;
        }
        // Whatever a post queued as the loop was ending is still written.
        self.flush_queue(&branch).await?;
        outcome.stopped_early = !replied;
        Ok(outcome)
    }

    /// Execute one tool call: tool.pre dispatch (block/rewrite), routing to
    /// the owning plugin, control validation. `Ok(None)` = aborted mid-call.
    async fn exec_tool_call(
        self: &Arc<Self>,
        branch: &BranchId,
        session: &SessionId,
        call: &ToolCall,
        token: &CancellationToken,
        caller: Caller<'_>,
        depth: usize,
    ) -> Result<Option<ExecutedTool>> {
        let started = Instant::now();
        let mut payload = json!({ "name": call.name, "args": call.args });
        let parent_call = match caller {
            Caller::Model => None,
            Caller::Plugin { name, parent_call } => {
                payload["caller"] = json!(name);
                payload["parent_call"] = json!(parent_call);
                Some(parent_call)
            }
        };
        // tool.pre: interceptable.
        let mut ev = self.make_event(kind::TOOL_PRE, session.clone(), branch.clone(), payload)?;
        let mut effect_source: Option<String> = None;
        let (out, blocked) = match self.dispatch(&mut ev, true).await? {
            Some(reason) => (
                ToolOutput::err(format!("Blocked by hook: {reason}")),
                Some(reason),
            ),
            None => {
                let name = ev
                    .payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&call.name)
                    .to_owned();
                let mut args = ev
                    .payload
                    .get("args")
                    .cloned()
                    .unwrap_or_else(|| call.args.clone());
                // A sub-agent runs in the background unless the model asks to
                // wait on it; other tools wait unless asked otherwise.
                let wants_background = args
                    .get("background")
                    .and_then(Value::as_bool)
                    .unwrap_or(name == "agent" && matches!(caller, Caller::Model));
                let registry = self.registry();
                let background_tool = registry
                    .tools
                    .iter()
                    .any(|t| t.name == name && t.background);
                // A script's call hangs from its enclosing `code` call: that
                // is the main-line call a task or forked agent anchors to.
                let (outer_call, inner_call) = match caller {
                    Caller::Model => (call.call_id.as_str(), None),
                    Caller::Plugin { parent_call, .. } => {
                        (parent_call, Some(call.call_id.as_str()))
                    }
                };
                let strip_background = |args: &mut Value| {
                    if let Some(obj) = args.as_object_mut() {
                        obj.remove("background");
                    }
                };
                // A rewrite may change the name; the denied set still applies
                // to what will actually run (no nested plugin invokes of a
                // denied tool), and no controls.
                let out = if matches!(caller, Caller::Plugin { .. })
                    && CODE_DENIED_TOOLS.contains(&name.as_str())
                {
                    ToolOutput::err(format!(
                        "tool `{name}` is not available from a plugin invoke"
                    ))
                } else if name == "agent" {
                    // Kernel built-in (not routed to a plugin). The sub-agent
                    // owns its cancellation: the child branch's token is a
                    // child of ours, so an abort lets `run_agent` finish
                    // (marking the child Done) instead of dropping the future
                    // mid-flight and leaving the branch Active forever.
                    match parse_args::<AgentArgs>("agent", &args)
                        .and_then(|args| args.into_parts().map_err(ToolError::model))
                    {
                        Err(e) => ToolOutput::err(e),
                        Ok((task, inherit, ask)) => {
                            if wants_background {
                                match self.start_agent_task(
                                    branch, session, outer_call, inner_call, task, inherit, ask,
                                ) {
                                    Ok(out) => out,
                                    Err(e) => ToolOutput::err(format!("agent failed: {e}")),
                                }
                            } else {
                                let (fork_call, inner_call) = (outer_call, inner_call);
                                match Box::pin(self.run_agent(
                                    branch, fork_call, inner_call, &task, inherit, ask, token, None,
                                ))
                                .await
                                {
                                    Ok(AgentRun::Summary(text)) => ToolOutput::ok(text),
                                    Ok(AgentRun::Interrupted(text)) => ToolOutput::err(text),
                                    Ok(AgentRun::Aborted) => ToolOutput::err("aborted"),
                                    Ok(AgentRun::Refused(message)) => ToolOutput::err(message),
                                    Err(e) => ToolOutput::err(format!("agent failed: {e}")),
                                }
                            }
                        }
                    }
                } else {
                    // `background` is a kernel affordance on tools that
                    // advertise it; never leak it into the plugin's args.
                    let summary = if background_tool {
                        args_summary(&name, &args)
                    } else {
                        String::new()
                    };
                    if background_tool {
                        strip_background(&mut args);
                    }
                    if wants_background && background_tool {
                        match self.start_tool_task(
                            branch,
                            session,
                            outer_call,
                            &call.call_id,
                            &name,
                            args,
                            summary,
                        ) {
                            Ok(out) => out,
                            Err(e) => ToolOutput::err(format!("starting task failed: {e}")),
                        }
                    } else {
                        match registry.routes.get(&name).cloned() {
                            None => ToolOutput::err(format!("no plugin provides tool `{name}`")),
                            Some(route) => {
                                let (plugin, source) = (route.plugin, route.name);
                                // A model's call of a background-capable tool
                                // moves to the background at its limit; a
                                // script's stops there, when the tool takes a
                                // `timeout` (else the script's own bounds it).
                                let model = matches!(caller, Caller::Model);
                                let limit = background_tool
                                    .then(|| {
                                        foreground_limit(&args, model || self.takes_timeout(&name))
                                    })
                                    .flatten();
                                let mut sink = self.tool_progress_sink(branch, &call.call_id);
                                let handoff = (model && limit.is_some()).then(|| {
                                    let handoff = Handoff::new(sink.clone());
                                    sink = handoff.sink(self);
                                    handoff
                                });
                                let cx = Cx::new(
                                    session.clone(),
                                    branch.clone(),
                                    Some(Call::new(
                                        call.call_id.clone(),
                                        parent_call.map(str::to_owned),
                                        false,
                                        sink,
                                    )),
                                    Some(source.clone()),
                                    depth,
                                    Arc::new(KernelRuntime(self.clone())),
                                );
                                let tool = name.clone();
                                let work: ToolWork =
                                    Box::pin(
                                        async move { plugin.call_tool(&tool, args, &cx).await },
                                    );
                                let foreground = Foreground {
                                    session: session.clone(),
                                    branch: branch.clone(),
                                    call_id: call.call_id.clone(),
                                    name: name.clone(),
                                    summary,
                                    source: source.clone(),
                                    handoff,
                                };
                                let Some(out) =
                                    self.run_foreground(work, limit, foreground, token).await
                                else {
                                    return Ok(None);
                                };
                                effect_source = Some(source);
                                out
                            }
                        }
                    }
                };
                (out, None)
            }
        };
        // A tool's returned effects land at the next safe point (or
        // immediately outside a batch), so later calls of this batch see them.
        if let Some(source) = effect_source {
            self.deliver_effects(branch, &source, out.effects.clone())
                .await?;
        }
        // The enclosing `code` call owns the per-parent seq counter; drop it
        // once that outer call is completely finished (all inner calls done).
        // The model sees the boundary cap; a plugin invoke gets a much larger
        // one, since the plugin (a `code` script) filters before the model.
        let out = match caller {
            Caller::Model => {
                let mut inner = self.inner.lock().unwrap();
                inner.inner_seq.remove(&call.call_id);
                inner.inner_scope.remove(&call.call_id);
                out.cap()
            }
            Caller::Plugin { .. } => out.cap_invoke(),
        };
        Ok(Some(ExecutedTool {
            out,
            payload: ev.payload,
            dur_ms: started.elapsed().as_millis() as u64,
            blocked,
            workspace: ev.fields.workspace,
        }))
    }

    /// Run a tool on behalf of a plugin (`Cx::invoke`). Dispatches
    /// `tool.pre` / `tool.post` exactly like a model call; appends made by
    /// hooks or plugins are queued for the loop's next safe point. Requires a
    /// running loop on the branch. The result lands as a trace-only
    /// [`NodeBody::ToolResult`] node under `parent_call`, appended on
    /// completion (so concurrent invokes chain in completion order).
    /// Background tasks are refused by `Cx::invoke` before reaching this.
    pub(crate) async fn exec_invoke(
        self: &Arc<Self>,
        branch: &BranchId,
        caller: &str,
        parent_call: &str,
        depth: usize,
        name: &str,
        args: Value,
    ) -> Result<InnerCall> {
        // Bound invoke recursion: A invokes B invokes A…
        if depth > MAX_INVOKE_DEPTH {
            return Ok(InnerCall {
                output: ToolOutput::err(format!(
                    "invoke depth limit of {MAX_INVOKE_DEPTH} exceeded"
                )),
                blocked: None,
                dur_ms: 0,
            });
        }
        // Denied tools are rejected before any hook runs — no `tool.pre`,
        // no `tool.post`, no trace row.
        if CODE_DENIED_TOOLS.contains(&name) {
            return Ok(InnerCall {
                output: ToolOutput::err(format!(
                    "tool `{name}` is not available from a plugin invoke"
                )),
                blocked: None,
                dur_ms: 0,
            });
        }
        let session = self.session_of(branch)?;
        let (token, call_id) = {
            let mut inner = self.inner.lock().unwrap();
            let running =
                inner
                    .running
                    .get(branch)
                    .cloned()
                    .ok_or_else(|| Error::NoRunningTurn {
                        branch: branch.clone(),
                    })?;
            let token = inner
                .inner_scope
                .entry(parent_call.to_owned())
                .or_insert_with(|| running.child_token())
                .clone();
            let n = inner.inner_seq.entry(parent_call.to_owned()).or_insert(0);
            *n += 1;
            (token, format!("{parent_call}.{n}"))
        };
        let call = ToolCall::new(call_id, name, args.clone());
        let Some(ExecutedTool {
            out,
            mut payload,
            dur_ms,
            blocked,
            workspace,
        }) = self
            .exec_tool_call(
                branch,
                &session,
                &call,
                &token,
                Caller::Plugin {
                    name: caller,
                    parent_call,
                },
                depth,
            )
            .await?
        else {
            return Err(Error::InternalAborted);
        };
        let effective_name = payload
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_owned();
        let effective_args = payload.get("args").cloned().unwrap_or(args);
        payload["output"] = json!(out.content.text);
        payload["is_error"] = json!(out.is_error);
        let mut ev = self.make_event(kind::TOOL_POST, session.clone(), branch.clone(), payload)?;
        self.dispatch(&mut ev, false).await?;

        // The node keeps what the model would have seen for a direct call;
        // the script already got the full output up to the invoke cap. Only
        // the file patch (else the workspace addressed) is kept: it lets a
        // reloaded session render an internal call exactly like a direct one.
        self.append_body_dur(
            branch,
            NodeBody::ToolResult {
                call_id: call.call_id.clone(),
                content: Content {
                    text: truncate_output(out.content.text.clone()),
                    attachments: Vec::new(),
                    data: with_target(
                        out.content.data.clone().filter(|d| d.tag == FILE_PATCH_TAG),
                        workspace,
                    ),
                },
                is_error: out.is_error,
                invoke: Some(Invoke {
                    parent: parent_call.to_owned(),
                    name: effective_name,
                    args: effective_args,
                    blocked: blocked.clone(),
                }),
            },
            None,
            Some(dur_ms),
        )
        .await?;

        Ok(InnerCall {
            output: out,
            blocked,
            dur_ms,
        })
    }

    /// The built-in `agent` tool: fork a child branch, seed it with the
    /// inherited context (or just the task), run the child loop, and return
    /// the summary.
    ///
    /// The sub-agent ends when it replies without tool calls — that reply is
    /// the summary. An abort without one yields a fixed stopper line (plus
    /// the last assistant text, if any). The result also carries the address
    /// of the child's branch (`@rumu:/sessions/<s>/branches/<b>`): `read`
    /// opens its transcript, `branch.send` messages it. A background
    /// sub-agent's task (`task_id`) names the branch as soon as it exists.
    ///
    /// With mode `fork` the fork point is the assistant node containing
    /// `call_id` (found by walking the path, not by assuming it is the head:
    /// inside a tool batch earlier calls have already appended ToolResults,
    /// and a script's agent forks at the enclosing `code` call; `inner_call`
    /// is then the internal call recorded on the branch). With mode
    /// `fresh` it forks at the session root and seeds only markers plus the
    /// task.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_agent(
        self: &Arc<Self>,
        from: &BranchId,
        call_id: &str,
        inner_call: Option<&str>,
        task: &str,
        inherit: Inherit,
        ask: ModelAsk,
        parent_token: &CancellationToken,
        task_id: Option<&crate::TaskId>,
    ) -> Result<AgentRun> {
        let parent = self
            .store
            .get_branch(from)?
            .ok_or_else(|| Error::BranchNotFound(from.clone()))?;
        let path = self.path_nodes(from)?;
        let parent_state = self.state_of_nodes(&self.pending_path_nodes(from)?);

        // The child's workspaces: the parent's, or the named subset of them.
        // A name the parent does not have never starts a child.
        let available: Vec<String> = parent_state
            .workspaces
            .iter()
            .map(|w| w.name.clone())
            .collect();
        if let Some(names) = &inherit.workspaces
            && let Some(name) = names
                .iter()
                .find(|n| *n != RUMU_WORKSPACE && !available.contains(n))
        {
            return Ok(AgentRun::Refused(format!(
                "agent: unknown workspace `{name}` (available: {})",
                available.join(", ")
            )));
        }
        let child_workspaces: Option<Vec<String>> = inherit.workspaces.as_ref().map(|names| {
            available
                .iter()
                .filter(|n| names.contains(n))
                .cloned()
                .collect()
        });

        // Depth is computed from the Agent ancestry of the branch we are
        // forking from; 4th level and beyond is refused.
        let depth_limit = self.inner.lock().unwrap().agent_depth;
        if self.agent_depth(from)? >= depth_limit {
            return Ok(AgentRun::Refused(
                "agent depth limit reached; do the work yourself".into(),
            ));
        }

        // The model it asks for. With the whole context it stays on this
        // model: another one could not take this path's reasoning or cache.
        let parent_model = self.model_of(&parent_state);
        let model = match self.model_override(ask.model, ask.effort, parent_model.as_ref()) {
            Ok(m) => m,
            Err(e) => return Ok(AgentRun::Refused(e)),
        };
        if let (InheritContext::Full, Some(m)) = (&inherit.context, &model)
            && self.llm.target(ModelPick::Use(m))
                != self.llm.target(crate::compact::pick(&parent_model))
        {
            return Ok(AgentRun::Refused(
                "agent: with mode \"fork\" a sub-agent stays on this model (only `effort` may \
                 change); use mode \"fresh\" for another model"
                    .into(),
            ));
        }

        // Fork point plus the child's first own nodes. Both context modes
        // are expressed this way so future modes can add their own seeding.
        let (fork, initial) = match inherit.context {
            InheritContext::Full => {
                let head = path
                    .iter()
                    .rev()
                    .find(|n| {
                        matches!(&n.body, NodeBody::Assistant { tool_calls, .. }
                            if tool_calls.iter().any(|c| c.call_id == call_id))
                    })
                    .ok_or_else(|| Error::AgentCallNotFound {
                        call_id: call_id.to_owned(),
                        branch: from.clone(),
                    })?;
                let NodeBody::Assistant { tool_calls, .. } = &head.body else {
                    unreachable!("found a non-assistant node");
                };
                let mut initial = Vec::new();
                if let Some(names) = child_workspaces {
                    initial.push(set_marker(names));
                }
                if let Some(m) = model.filter(|m| Some(m) != parent_state.model.as_ref()) {
                    initial.push(NodeBody::Note {
                        source: SYSTEM_SOURCE.into(),
                        content: Content::marker(&m),
                    });
                }
                // The batch is answered by placeholders that say whose it is,
                // and the goal comes as a prompt of its own: delivered as the
                // `agent` call's result, it read as the parent's call coming
                // back, and the child carried on with the parent's plan (and
                // took its sibling calls' placeholders for failures).
                for call in tool_calls {
                    let text = if call.call_id == call_id {
                        "(handed to the sub-agent; its goal follows)"
                    } else {
                        "(left to the parent; not part of the sub-agent's work)"
                    };
                    initial.push(NodeBody::ToolResult {
                        call_id: call.call_id.clone(),
                        content: Content::text(text),
                        is_error: false,
                        invoke: None,
                    });
                }
                initial.push(NodeBody::User {
                    content: Content::text(format!(
                        "{}\nThe conversation above is your parent's, kept for context: its plan \
                         and its other calls are not yours to carry on. Work only on the goal.",
                        sub_agent_text(task)
                    )),
                });
                (head.id.clone(), initial)
            }
            InheritContext::None => {
                let root = path
                    .first()
                    .ok_or_else(|| Error::Internal(format!("branch {from} has no root node")))?;
                // Forked at the root: the parent's current state, not the
                // root's, is the starting point. Markers only where the root
                // alone would differ: the restriction, the project, the
                // default workspace.
                let mut initial = Vec::new();
                if let Some(names) = child_workspaces {
                    initial.push(set_marker(names));
                } else if parent_state.restrict.is_some() {
                    initial.push(set_marker(available.clone()));
                }
                // The parent's project where the root's differs: the root's
                // goes, then the parent's comes at its point (only the
                // project: other decks the parent mounted stay its own).
                let at_root = crate::state::derive_state(std::slice::from_ref(root));
                if parent_state.project_mount() != at_root.project_mount() {
                    let gone = at_root
                        .project_mount()
                        .map(|m| Content::marker(&rumu_core::CardUnmount { at: m.at.clone() }));
                    let added = parent_state.project_mount().map(Content::marker);
                    initial.extend(gone.into_iter().chain(added).map(|content| NodeBody::Note {
                        source: SYSTEM_SOURCE.into(),
                        content,
                    }));
                }
                // Keep the parent's default workspace when the child can
                // still address it; otherwise the child's merge picks one.
                let seeded: Vec<rumu_core::Node> = std::iter::once(root.clone())
                    .chain(initial.iter().map(|body| rumu_core::Node {
                        body: body.clone(),
                        ..root.clone()
                    }))
                    .collect();
                let natural = self.state_of_nodes(&seeded);
                let keep = parent_state.default_workspace.clone().filter(|n| {
                    natural.default_workspace.as_ref() != Some(n)
                        && (n == RUMU_WORKSPACE || natural.workspaces.iter().any(|w| &w.name == n))
                });
                if let Some(name) = keep {
                    initial.push(NodeBody::Note {
                        source: SYSTEM_SOURCE.into(),
                        content: Content::marker(&WorkspaceUse { name }),
                    });
                }
                // The child runs on the model it asked for, else the parent's.
                if let Some(model) = model.as_ref().or(parent_state.model.as_ref()) {
                    initial.push(NodeBody::Note {
                        source: SYSTEM_SOURCE.into(),
                        content: Content::marker(model),
                    });
                }
                initial.push(NodeBody::User {
                    content: Content::text(sub_agent_text(task)),
                });
                (root.id.clone(), initial)
            }
        };

        // Concurrency gate: excess sub-agents wait for a permit, and an abort
        // while waiting must not start a child at all.
        let gate = self.inner.lock().unwrap().agent_gate.clone();
        let _permit = tokio::select! {
            biased;
            _ = parent_token.cancelled() => return Ok(AgentRun::Aborted),
            permit = gate.acquire_owned() => {
                permit.map_err(|_| Error::Internal("agent gate closed".into()))?
            }
        };

        // The child token is created from the parent loop's token before the
        // branch is published, so an abort from here on propagates.
        let token = parent_token.child_token();

        let child = Self::make_branch(
            &parent.session,
            fork,
            BranchKind::Agent {
                task: task.to_owned(),
                parent_branch: from.clone(),
                call_id: inner_call.unwrap_or(call_id).to_owned(),
                via_plugin: inner_call.is_some(),
                inherit: inherit.clone(),
            },
        );
        self.store.insert_branch(&child)?;
        self.push("branch.updated", json!({ "branch": child }));
        // A background sub-agent's task names its branch from now on, so it
        // can be messaged while it works.
        if let Some(id) = task_id {
            self.set_task_child(id, &child.id);
        }

        for body in initial {
            self.append_body(&child.id, body, None).await?;
        }

        // The child runs concurrently with other branches; its token is a
        // child of the parent loop's token (if any), so an abort propagates.
        {
            let mut inner = self.inner.lock().unwrap();
            inner.running.insert(child.id.clone(), token.clone());
        }
        self.refresh_status_of(&child.id, false);
        self.push("turn.started", json!({ "branch": child.id }));

        let looped: Result<LoopOutcome> = async {
            loop {
                let outcome = self.run_loop(child.id.clone(), &token).await?;
                if token.is_cancelled() {
                    return Ok(outcome);
                }
                // A sub-agent is not finished while it still owns running tasks.
                // Wait for them; their completion posts land on the child branch
                // (registered as running for the whole `run_agent`) and are
                // consumed by another loop iteration instead of a wake turn.
                let owns_tasks = self.branch_has_running_tasks(&child.id);
                if owns_tasks {
                    self.wait_branch_tasks(&child.id, &token).await?;
                }
                let pending_wake = {
                    let inner = self.inner.lock().unwrap();
                    inner.wake_pending.get(&child.id).is_some_and(|p| p.active)
                };
                if !owns_tasks && !pending_wake {
                    return Ok(outcome);
                }
            }
        }
        .await;
        // A failed loop is closed like a failed turn: recorded on the branch,
        // the loop guard released and `turn.error` pushed — else the branch
        // stays busy for good, with nothing left to stop. Its tasks go too.
        let outcome = match looped {
            Ok(outcome) => outcome,
            Err(e) => {
                self.set_branch_status(&child.id, BranchStatus::Done);
                self.finish_turn(child.id.clone(), Some(&e), None).await;
                self.abort_tasks(&child.id, true, CancelSource::SubAgent);
                return Err(e);
            }
        };

        // Done before the guard goes: a message from here on reopens the
        // branch, and that turn's reply reaches the parent as a note.
        self.set_branch_status(&child.id, BranchStatus::Done);
        // Safe point at the end of the loop; also releases the loop guard and
        // re-arms a wake that arrived as the loop ended.
        self.end_loop(&child.id).await;
        // A finished sub-agent waited for its tasks above. One stopped by
        // itself is stopped as the main line is: its tasks run on, and their
        // notes wake it again, its replies reaching the parent as notes. One
        // cancelled with its parent (the task killed, the parent's turn
        // stopped) takes them down with it.
        if parent_token.is_cancelled() {
            self.abort_tasks(&child.id, true, CancelSource::SubAgent);
        }

        let b = self
            .store
            .get_branch(&child.id)?
            .ok_or_else(|| Error::Internal("agent branch vanished while finishing".into()))?;
        self.push("turn.done", json!({ "branch": child.id }));

        // Cancelled here but not above: the user stopped this sub-agent itself.
        let interrupted = token.is_cancelled();
        let header = if !interrupted {
            None
        } else if parent_token.is_cancelled() {
            Some("(sub-agent cancelled before it finished)")
        } else if self.branch_has_running_tasks(&child.id) {
            Some(
                "(sub-agent interrupted by the user before it finished; its background tasks \
                 keep running, and their results wake it again)",
            )
        } else {
            Some(INTERRUPTED)
        };
        let summary = agent_summary(&outcome, header);
        let text = format!(
            "{summary}\n\n[sub-agent: {} — `read` it for its transcript, `invoke` `branch.send` \
             on it to message it; its replies come back here]",
            rumu_core::branch_address(&parent.session, &b.id)
        );
        Ok(if interrupted {
            AgentRun::Interrupted(text)
        } else {
            AgentRun::Summary(text)
        })
    }

    /// The nesting depth of `branch`: how many `Agent` branches are in its
    /// ancestry (main = 0).
    fn agent_depth(&self, branch: &BranchId) -> Result<usize> {
        let mut b = self
            .store
            .get_branch(branch)?
            .ok_or_else(|| Error::BranchNotFound(branch.clone()))?;
        let mut depth = 0;
        while let BranchKind::Agent { parent_branch, .. } = b.kind.clone() {
            depth += 1;
            b = self
                .store
                .get_branch(&parent_branch)?
                .ok_or_else(|| Error::BranchNotFound(parent_branch.clone()))?;
        }
        Ok(depth)
    }
}

/// What a sub-agent run produced.
pub(crate) enum AgentRun {
    /// Its reply, and the address of its branch.
    Summary(String),
    /// The child branch was cancelled before it finished (the user stopped
    /// it, or its parent / task was cancelled): its partial summary.
    Interrupted(String),
    /// The parent was aborted while waiting for a concurrency permit.
    Aborted,
    /// A limit refused the call before any child branch existed.
    Refused(String),
}

impl Kernel {
    /// Does tool `name` take a `timeout` argument?
    fn takes_timeout(&self, name: &str) -> bool {
        self.registry()
            .tools
            .iter()
            .any(|t| t.name == name && t.schema["properties"].get("timeout").is_some())
    }
}

/// A short, single-line summary of a background tool's arguments.
fn args_summary(name: &str, args: &Value) -> String {
    let text = match name {
        // a bash call's description names it better than its command
        "bash" => ["description", "command"]
            .iter()
            .filter_map(|key| args.get(*key).and_then(Value::as_str))
            .find(|text| !text.trim().is_empty())
            .unwrap_or_default(),
        "agent" => args.get("task").and_then(Value::as_str).unwrap_or_default(),
        _ => "",
    };
    let text = if text.is_empty() {
        serde_json::to_string(args).unwrap_or_default()
    } else {
        text.to_owned()
    };
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ; ");
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentArgs {
    /// what the sub-agent should do, 1-2 sentences
    task: String,
    /// fresh (default): a clean branch here that sees only the task; fork: continue from here with this whole conversation
    mode: Option<Mode>,
    /// a model alias (`rumu/<name>`) to run on; not with fork
    model: Option<String>,
    /// the reasoning effort to run at
    effort: Option<Effort>,
    /// "all" (default) or an array of workspace names it may use
    workspaces: Option<Workspaces>,
    /// true (default): run in the background, returning a task id now and a notification when it finishes; false: wait here for its summary
    background: Option<bool>,
}

impl AgentArgs {
    /// Split into the run's `(task, inherit, model)`, rejecting a blank task.
    fn into_parts(self) -> std::result::Result<(String, Inherit, ModelAsk), &'static str> {
        // `background` is a kernel affordance acted on before this point; it
        // is parsed only so `deny_unknown_fields` accepts it.
        let _ = self.background;
        let task = self.task.trim();
        if task.is_empty() {
            return Err("agent: `task` must not be empty");
        }
        let context = match self.mode.unwrap_or_default() {
            Mode::Fork => InheritContext::Full,
            Mode::Fresh => InheritContext::None,
        };
        let workspaces = match self.workspaces {
            Some(Workspaces::Names(names)) => Some(names),
            Some(Workspaces::All(_)) | None => None,
        };
        Ok((
            task.to_owned(),
            Inherit {
                context,
                workspaces,
            },
            ModelAsk {
                model: self.model,
                effort: self.effort,
            },
        ))
    }
}

/// The model an `agent` call asks for: an alias, and the effort.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModelAsk {
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

impl Kernel {
    /// What a sub-agent asking for `model` (an alias) at `effort` runs on:
    /// that alias, or `base`'s model at `effort`; `None` when it asks for
    /// neither. `Err` is the refusal.
    pub(crate) fn model_override(
        &self,
        model: Option<String>,
        effort: Option<Effort>,
        base: Option<&ModelUse>,
    ) -> std::result::Result<Option<ModelUse>, String> {
        let model = match model {
            Some(m) => {
                let aliases = self.llm.aliases();
                if !aliases.contains(&m) {
                    return Err(format!(
                        "agent: `{m}` is not a model alias (aliases: {})",
                        if aliases.is_empty() {
                            "none".into()
                        } else {
                            aliases.join(", ")
                        }
                    ));
                }
                m
            }
            None if effort.is_none() => return Ok(None),
            None => base
                .map(|b| b.model.clone())
                .ok_or("agent: no model to set `effort` on; the assistant has none")?,
        };
        Ok(Some(ModelUse { model, effort }))
    }
}

// Where an `agent` runs: `Fork` and `Fresh` are branches of this session
// (`rumu_core::InheritContext::Full` / `None`), `Session` a session of its own.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Fork,
    #[default]
    Fresh,
}

// `"all"` or an explicit workspace-name list.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum Workspaces {
    All(AllLiteral),
    Names(Vec<String>),
}

#[derive(Debug, Deserialize, JsonSchema)]
enum AllLiteral {
    #[serde(rename = "all")]
    All,
}

/// A `workspace.set` marker body.
fn set_marker(names: Vec<String>) -> NodeBody {
    NodeBody::Note {
        source: SYSTEM_SOURCE.into(),
        content: Content::marker(&WorkspaceSet { names }),
    }
}

/// The `prompt` event payload: the text (the only part a hook rewrite may
/// change) plus the serialized attachments, so hooks can see them.
fn prompt_payload(content: &Content) -> Value {
    json!({ "text": content.text, "attachments": content.attachments })
}

/// The prompt content after dispatch: a rewritten `text`, when a hook changed
/// it, over the original attachments — a rewrite cannot touch those.
fn rewritten(ev: &rumu_core::Event, mut content: Content) -> Content {
    if let Some(text) = ev.payload.get("text").and_then(Value::as_str) {
        content.text = text.to_owned();
    }
    content
}

/// What a sub-agent's turn hands back: its last text, after a header when the
/// turn did not end on a reply. `header` names why it was cut short; without
/// one, a loop that stopped early gets the generic line.
pub(crate) fn agent_summary(outcome: &LoopOutcome, header: Option<&str>) -> String {
    let header = header.or_else(|| {
        outcome
            .stopped_early
            .then_some("(sub-agent stopped without a final reply)")
    });
    match header {
        Some(header) if outcome.last_text.is_empty() => header.to_owned(),
        Some(header) => format!("{header}\n{}", outcome.last_text),
        None => outcome.last_text.clone(),
    }
}

/// The header of a sub-agent turn the user stopped.
pub(crate) const INTERRUPTED: &str = "(sub-agent interrupted by the user before it finished)";

/// The framing both context modes give a sub-agent.
fn sub_agent_text(task: &str) -> String {
    format!(
        "You are now a sub-agent. Goal: {task}\n\
         Do the work, then reply with a concise summary — a reply without tool calls ends your \
         turn and goes back to your parent; nothing else you do here is visible to it. Messages \
         may come later; each reply to them goes back the same way."
    )
}
