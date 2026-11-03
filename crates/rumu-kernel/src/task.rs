//! Background tasks: kernel-owned, runtime-only table, completion via the
//! inbox, and the built-in `task_kill` tool.
//!
//! A task is a plugin tool call or a sub-agent that runs detached from the
//! turn that started it. The kernel keeps its status and an output buffer in
//! [`Inner::tasks`](crate::Inner); the buffer keeps the last
//! [`TASK_OUTPUT_LIMIT`] bytes of `Progress::Append` text plus the latest
//! `Progress::Data`. Nothing is persisted: a kernel restart loses every
//! running task (and [`Kernel::new`] closes out their agent branches).

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::{Error, Kernel, PostRequest, Result, TASK_OUTPUT_LIMIT, turn::AgentRun};
use rumu_core::{
    BranchId, Call, Content, Cx, Effect, Post, Progress, ProgressSink, RUMU_WORKSPACE,
    SYSTEM_SOURCE, SessionId, TaskDone, TaskId, TaskInfo, TaskKind, TaskStatus, ToolError,
    ToolOutput, ToolResult, now, parse_args, task_output_hint,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::host::KernelRuntime;

/// One task's runtime row. Tasks stay in the table after they end so the UI
/// and `@rumu:/tasks/<id>` can still show the result.
pub(crate) struct TaskEntry {
    pub info: TaskInfo,
    /// Buffered `Progress::Append` text, capped to the last `TASK_OUTPUT_LIMIT`
    /// bytes.
    pub output: String,
    /// Bytes dropped from the front of `output`.
    pub dropped: u64,
    /// The plugin/`kernel` name that owns the tool's effects.
    pub source: String,
    /// Cancels the work (tool future dropped, agent token cancelled).
    pub token: CancellationToken,
    /// The completion post has been queued (a waiting sub-agent may loop).
    pub post_done: bool,
    /// Cancelled by [`crate::Kernel::abort`] for its owner: report with
    /// `wake: false`.
    pub aborted_by_owner: bool,
}

/// What a task's work returned.
pub(crate) enum TaskOutcome {
    Result(Result<ToolOutput>),
    Cancelled,
}

impl Kernel {
    /// Every task owned by a branch of `session`, oldest first.
    pub fn tasks(&self, session: &SessionId) -> Result<Vec<TaskInfo>> {
        let inner = self.inner.lock().unwrap();
        let mut tasks: Vec<TaskInfo> = inner
            .tasks
            .values()
            .filter(|e| &e.info.session == session)
            .map(|e| e.info.clone())
            .collect();
        tasks.sort_by(|a, b| {
            a.started_at
                .cmp(&b.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(tasks)
    }

    /// One task plus its rendered output buffer.
    pub fn task(&self, id: &TaskId) -> Result<Option<(TaskInfo, String)>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.tasks.get(id).map(|e| {
            let mut output = String::new();
            if e.dropped > 0 {
                output.push_str(&format!(
                    "… [truncated, {} earlier bytes omitted]\n",
                    e.dropped
                ));
            }
            output.push_str(&e.output);
            (e.info.clone(), output)
        }))
    }

    /// Cancel a task by id (the UI's `task.kill`, no ownership check) and
    /// wait briefly for its final status.
    pub async fn kill_task(&self, id: &TaskId) -> Result<TaskInfo> {
        let Some((token, _)) = self.task_token(id) else {
            return Err(Error::TaskNotFound { id: id.clone() });
        };
        token.cancel();
        self.wait_final(id, Duration::from_secs(5))
            .await
            .ok_or_else(|| Error::TaskNotFound { id: id.clone() })
    }

    fn task_token(&self, id: &TaskId) -> Option<(CancellationToken, TaskInfo)> {
        let inner = self.inner.lock().unwrap();
        inner
            .tasks
            .get(id)
            .map(|e| (e.token.clone(), e.info.clone()))
    }

    /// Wake every waiter: a task's status or `post_done` changed.
    fn signal_tasks(&self) {
        self.task_changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// The task's final snapshot, or its current one when `timeout` expires.
    /// `None` when no such task exists.
    async fn wait_final(&self, id: &TaskId, timeout: Duration) -> Option<TaskInfo> {
        let deadline = Instant::now() + timeout;
        let mut changed = self.task_changed.subscribe();
        loop {
            let info = {
                let inner = self.inner.lock().unwrap();
                inner.tasks.get(id).map(|e| e.info.clone())
            };
            match info {
                Some(info) if !info.status.is_running() => return Some(info),
                Some(info) if Instant::now() >= deadline => return Some(info),
                Some(_) => {}
                None => return None,
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                _ = changed.changed() => {}
                _ = tokio::time::sleep(remaining) => {}
            }
        }
    }

    // ---- starting ----

    /// Start a plugin tool as a background task.
    pub(crate) fn start_tool_task(
        self: &Arc<Self>,
        branch: &BranchId,
        session: &SessionId,
        call_id: &str,
        name: &str,
        args: Value,
        args_summary: String,
    ) -> Result<ToolOutput> {
        let Some(&plugin_index) = self.routes.get(name) else {
            return Ok(ToolOutput::err(format!("no plugin provides tool `{name}`")));
        };
        let plugin = self.plugins[plugin_index].clone();
        let source = self.plugin_names[plugin_index].clone();
        let kind = TaskKind::Tool {
            name: name.to_owned(),
            args_summary: args_summary.clone(),
        };
        let label = kind.label("");
        let (id, token) =
            match self.create_task(session, branch, call_id, kind, args_summary, source.clone()) {
                Ok(v) => v,
                Err(e) => return Ok(ToolOutput::err(e.to_string())),
            };

        // Progress feeds this task's buffer instead of the push stream.
        let sink: ProgressSink = {
            let kernel = self.clone();
            let id = id.clone();
            Arc::new(move |update| kernel.task_progress(&id, update))
        };
        let cx = Cx::new(
            session.clone(),
            branch.clone(),
            Some(Call::new(call_id.to_owned(), None, true, sink)),
            Some(source),
            0,
            Arc::new(KernelRuntime(self.clone())),
        );

        let kernel = self.clone();
        let task_id = id.clone();
        let name = name.to_owned();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                biased;
                _ = token.cancelled() => TaskOutcome::Cancelled,
                r = plugin.call_tool(&name, args, &cx) => {
                    TaskOutcome::Result(r.map_err(Error::from))
                }
            };
            kernel.finish_task(&task_id, outcome).await;
        });

        Ok(ToolOutput::ok(format!(
            "started task {id} ({label}) — you will be notified when it finishes; \
             use task_kill / read {RUMU_WORKSPACE}:/tasks/{id}"
        )))
    }

    /// Start a sub-agent as a background task. The agent permit is acquired
    /// inside the task, so the model gets its id immediately.
    pub(crate) fn start_agent_task(
        self: &Arc<Self>,
        branch: &BranchId,
        session: &SessionId,
        call_id: &str,
        task: String,
        inherit: rumu_core::Inherit,
    ) -> Result<ToolOutput> {
        let kind = TaskKind::Agent { child_branch: None };
        let label = format!("agent: {task}");
        let (id, token) = match self.create_task(
            session,
            branch,
            call_id,
            kind,
            task.clone(),
            SYSTEM_SOURCE.into(),
        ) {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(e.to_string())),
        };

        let kernel = self.clone();
        let task_id = id.clone();
        let owner = branch.clone();
        let fork_call = call_id.to_owned();
        tokio::spawn(async move {
            let outcome = match kernel
                .run_agent(&owner, &fork_call, None, &task, inherit, &token)
                .await
            {
                Ok(AgentRun::Summary { text, child }) => {
                    kernel.set_task_child(&task_id, &child);
                    TaskOutcome::Result(Ok(ToolOutput::ok(text)))
                }
                Ok(AgentRun::Aborted) => TaskOutcome::Cancelled,
                Ok(AgentRun::Refused(message)) => TaskOutcome::Result(Ok(ToolOutput::err(message))),
                Err(e) => TaskOutcome::Result(Ok(ToolOutput::err(format!("agent failed: {e}")))),
            };
            kernel.finish_task(&task_id, outcome).await;
        });

        Ok(ToolOutput::ok(format!(
            "started task {id} ({label}) — you will be notified when it finishes; \
             use task_kill / read {RUMU_WORKSPACE}:/tasks/{id}"
        )))
    }

    fn create_task(
        self: &Arc<Self>,
        session: &SessionId,
        branch: &BranchId,
        call_id: &str,
        kind: TaskKind,
        summary: String,
        source: String,
    ) -> Result<(TaskId, CancellationToken)> {
        let (id, token, info) = {
            let mut inner = self.inner.lock().unwrap();
            let running = inner
                .tasks
                .values()
                .filter(|e| &e.info.branch == branch && e.info.status.is_running())
                .count();
            if running >= inner.max_tasks_per_branch {
                return Err(Error::TooManyTasks {
                    running,
                    limit: inner.max_tasks_per_branch,
                });
            }
            // Random ids: regenerate on the (astronomically unlikely) collision
            // with a task still in the table.
            let id = loop {
                let id = TaskId::new();
                if !inner.tasks.contains_key(&id) {
                    break id;
                }
            };
            let token = CancellationToken::new();
            let info = TaskInfo {
                id: id.clone(),
                session: session.clone(),
                branch: branch.clone(),
                call_id: call_id.to_owned(),
                kind,
                summary,
                status: TaskStatus::Running,
                started_at: now(),
                ended_at: None,
                truncated: false,
                is_error: false,
                result: None,
                data: None,
            };
            inner.tasks.insert(
                id.clone(),
                TaskEntry {
                    info: info.clone(),
                    output: String::new(),
                    dropped: 0,
                    source,
                    token: token.clone(),
                    post_done: false,
                    aborted_by_owner: false,
                },
            );
            (id, token, info)
        };
        self.signal_tasks();
        self.push("task.started", json!({ "task": info }));
        Ok((id, token))
    }

    /// Record which branch a background agent got.
    fn set_task_child(&self, id: &TaskId, child: &BranchId) {
        let info = {
            let mut inner = self.inner.lock().unwrap();
            let Some(entry) = inner.tasks.get_mut(id) else {
                return;
            };
            entry.info.kind = TaskKind::Agent {
                child_branch: Some(child.clone()),
            };
            entry.info.clone()
        };
        self.push(
            "task.progress",
            json!({ "id": id, "branch": info.branch, "kind": info.kind }),
        );
    }

    // ---- progress + completion ----

    /// Feed one progress update into a task's buffer and push it to clients.
    fn task_progress(&self, id: &TaskId, update: Progress) {
        let (branch, append, data) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(entry) = inner.tasks.get_mut(id) else {
                return;
            };
            if !entry.info.status.is_running() {
                return;
            }
            let branch = entry.info.branch.clone();
            let mut append = None;
            let mut data = None;
            match update {
                Progress::Append(text) => {
                    entry.output.push_str(&text);
                    trim_buffer(entry);
                    append = Some(text);
                }
                Progress::Data(tagged) => {
                    entry.info.data = Some(tagged.clone());
                    data = Some(tagged);
                }
            }
            (branch, append, data)
        };
        let mut params = json!({ "id": id, "branch": branch });
        if let Some(append) = append {
            params["append"] = json!(append);
        }
        if let Some(data) = data {
            params["data"] = json!(data);
        }
        self.push("task.progress", params);
    }

    /// Mark a task finished and deliver its completion through the post
    /// pipeline.
    async fn finish_task(self: &Arc<Self>, id: &TaskId, outcome: TaskOutcome) {
        let (info, note, effects, wake, source) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(entry) = inner.tasks.get_mut(id) else {
                return;
            };
            if !entry.info.status.is_running() {
                return;
            }
            let (status, content, effects, is_error) = match outcome {
                TaskOutcome::Cancelled => (TaskStatus::Cancelled, Content::text(""), vec![], false),
                TaskOutcome::Result(Ok(out)) => {
                    // The completion note reaches the model: the boundary cap.
                    let out = out.cap();
                    (
                        if out.is_error {
                            TaskStatus::Failed
                        } else {
                            TaskStatus::Done
                        },
                        out.content,
                        out.effects,
                        out.is_error,
                    )
                }
                TaskOutcome::Result(Err(e)) => (
                    TaskStatus::Failed,
                    Content::text(format!("{e}")),
                    vec![],
                    true,
                ),
            };
            entry.info.status = status;
            entry.info.ended_at = Some(now());
            entry.info.is_error = is_error;
            entry.info.result = Some(content.clone());
            let wake = !entry.aborted_by_owner;
            entry.aborted_by_owner = false;
            let note = completion_text(&entry.info, &content);
            (
                entry.info.clone(),
                note,
                effects,
                wake,
                entry.source.clone(),
            )
        };

        self.signal_tasks();
        self.push("task.done", json!({ "task": info }));
        self.deliver_completion(&info, note, effects, &source, wake)
            .await;
    }

    /// Post the tool's own effects (attributed to its plugin) and then the
    /// kernel's completion note. The post worker is ordered, so the effects
    /// land first; only the note wakes the branch.
    async fn deliver_completion(
        self: &Arc<Self>,
        info: &TaskInfo,
        note: String,
        effects: Vec<Effect>,
        source: &str,
        wake: bool,
    ) {
        let note = Content::note_with(
            note,
            &TaskDone {
                id: info.id.clone(),
                status: info.status,
                kind: info.kind.clone(),
                is_error: info.is_error,
            },
        );
        let mut requests = Vec::new();
        if !effects.is_empty() {
            requests.push((source.to_owned(), effects, false));
        }
        requests.push((SYSTEM_SOURCE.to_owned(), vec![note], wake));
        let mut ack = None;
        for (source, effects, wake) in requests {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            let request = PostRequest {
                branch: info.branch.clone(),
                source,
                post: Post { effects, wake },
                ack: Some(ack_tx),
            };
            if self.post_tx.send(request).is_err() {
                self.warn(format!(
                    "task {} completion post failed: the post worker is not running",
                    info.id
                ));
                ack = None;
                break;
            }
            ack = Some(ack_rx);
        }
        // Wait until the worker has queued the note (a sub-agent waiting for
        // its tasks loops again only after this returns).
        if let Some(ack_rx) = ack {
            let _ = tokio::time::timeout(Duration::from_secs(10), ack_rx).await;
        }
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(entry) = inner.tasks.get_mut(&info.id) {
                entry.post_done = true;
            }
        }
        self.signal_tasks();
    }

    // ---- readiness / abort ----

    /// True when `branch` owns a still-running task.
    pub(crate) fn branch_has_running_tasks(&self, branch: &BranchId) -> bool {
        self.inner
            .lock()
            .unwrap()
            .tasks
            .values()
            .any(|e| &e.info.branch == branch && e.info.status.is_running())
    }

    /// Wait until every task owned by `branch` has ended and its completion
    /// has been posted. Cancellable by the turn's token.
    pub(crate) async fn wait_branch_tasks(
        &self,
        branch: &BranchId,
        token: &CancellationToken,
    ) -> Result<()> {
        let mut changed = self.task_changed.subscribe();
        loop {
            if token.is_cancelled() {
                return Ok(());
            }
            let pending = {
                let inner = self.inner.lock().unwrap();
                inner.tasks.values().any(|e| {
                    &e.info.branch == branch && (e.info.status.is_running() || !e.post_done)
                })
            };
            if !pending {
                return Ok(());
            }
            tokio::select! {
                _ = token.cancelled() => return Ok(()),
                _ = changed.changed() => {}
            }
        }
    }

    /// Abort-time cleanup: cancel every running task owned by `branch` or one
    /// of its sub-agent descendants. Their completion notes are still written,
    /// with `wake: false`.
    pub(crate) fn abort_tasks(&self, branch: &BranchId) {
        let subtree = self.branch_subtree(branch).unwrap_or_else(|_| {
            let mut set = std::collections::HashSet::new();
            set.insert(branch.clone());
            set
        });
        let mut inner = self.inner.lock().unwrap();
        for entry in inner.tasks.values_mut() {
            if subtree.contains(&entry.info.branch) && entry.info.status.is_running() {
                entry.aborted_by_owner = true;
                entry.token.cancel();
            }
        }
    }

    /// The branch itself plus every agent branch below it.
    pub(crate) fn branch_subtree(
        &self,
        branch: &BranchId,
    ) -> Result<std::collections::HashSet<BranchId>> {
        let session = self.session_of(branch)?;
        let branches = self.store.branches_of_session(&session)?;
        let mut set = std::collections::HashSet::new();
        set.insert(branch.clone());
        loop {
            let mut changed = false;
            for b in &branches {
                if let rumu_core::BranchKind::Agent { parent_branch, .. } = &b.kind
                    && set.contains(parent_branch)
                    && !set.contains(&b.id)
                {
                    set.insert(b.id.clone());
                    changed = true;
                }
            }
            if !changed {
                return Ok(set);
            }
        }
    }

    /// True when a call on `requester` may touch a task owned by `owner`
    /// (the same branch, or a descendant sub-agent branch).
    fn owns_task(&self, requester: &BranchId, owner: &BranchId) -> bool {
        if requester == owner {
            return true;
        }
        self.branch_subtree(requester)
            .is_ok_and(|set| set.contains(owner))
    }

    // ---- built-in tools ----

    /// `task_kill(id)` from a branch (ownership checked).
    pub(crate) async fn tool_task_kill(
        self: &Arc<Self>,
        branch: &BranchId,
        args: &Value,
    ) -> ToolResult<ToolOutput> {
        let args: TaskKillArgs = parse_args("task_kill", args)?;
        let id = TaskId::from(args.id.as_str());
        let Some((token, info)) = self.task_token(&id) else {
            return Err(ToolError::model(format!("no task `{id}`")));
        };
        if !self.owns_task(branch, &info.branch) {
            return Err(ToolError::model(format!(
                "task `{id}` is not owned by this branch or its sub-agents"
            )));
        }
        if info.status.is_running() {
            token.cancel();
        }
        let Some(final_info) = self.wait_final(&id, Duration::from_secs(5)).await else {
            return Err(ToolError::model(format!("no task `{id}`")));
        };
        Ok(ToolOutput::ok(format!(
            "task {id}: {}",
            final_info.status.label()
        )))
    }
}

// `task_kill` arguments (also the source of its schema).
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskKillArgs {
    /// task id, e.g. t-1a2b3c4d
    id: String,
}
/// Keep the last [`TASK_OUTPUT_LIMIT`] bytes, on a char boundary.
fn trim_buffer(entry: &mut TaskEntry) {
    if entry.output.len() <= TASK_OUTPUT_LIMIT {
        return;
    }
    let mut cut = entry.output.len() - TASK_OUTPUT_LIMIT;
    while cut < entry.output.len() && !entry.output.is_char_boundary(cut) {
        cut += 1;
    }
    entry.output.drain(..cut);
    entry.dropped += cut as u64;
    entry.info.truncated = true;
}

/// The completion note: one header line, then the (truncated) result.
fn completion_text(info: &TaskInfo, content: &Content) -> String {
    let mut text = format!(
        "task {} ({}) finished: {}",
        info.id,
        info.label(),
        info.status.label()
    );
    if !content.text.is_empty() {
        text.push('\n');
        text.push_str(&content.text);
        if content.text.chars().count() > rumu_core::OUTPUT_LIMIT || info.truncated {
            text.push('\n');
            text.push_str(&task_output_hint(&info.id));
        }
    } else if info.truncated {
        text.push('\n');
        text.push_str(&task_output_hint(&info.id));
    }
    text
}
