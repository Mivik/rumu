//! Background tasks: the kernel's live table, completion via the inbox,
//! cancellation (`task.kill`) and what a restart leaves of them.
//!
//! A task is a plugin tool call or a sub-agent that runs detached from the
//! turn that started it. The kernel keeps its status and an output buffer in
//! [`Inner::tasks`](crate::Inner); the buffer keeps the last
//! [`TASK_OUTPUT_LIMIT`] bytes of `Progress::Append` text plus the latest
//! `Progress::Data`. Both die with the process, but each task also leaves a
//! record in its session's journal, so [`Kernel::recover_tasks`] can bring
//! the table back: ended tasks as they ended, and the ones still running as
//! [`TaskStatus::Lost`], with the completion note they never got to post.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Error, Kernel, PostRequest, Result, TASK_OUTPUT_LIMIT, turn::AgentRun};
use rumu_core::{
    BranchId, Call, Content, Cx, Effect, Post, Progress, ProgressSink, RUMU_WORKSPACE,
    SYSTEM_SOURCE, SessionId, TaskDone, TaskId, TaskInfo, TaskKind, TaskStatus, ToolOutput, now,
    task_output_hint,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::host::KernelRuntime;

/// What a task that a restart killed says in its completion note.
const LOST_TASK: &str = "rumu restarted while this task was running, so the work it was doing \
                         and the output it had collected are gone. Start it again if you still \
                         need it.";

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
    /// Cancelled by [`crate::Kernel::abort`] or with its stopped sub-agent:
    /// the completion note is written with `wake: false`.
    pub quiet: bool,
}

/// A plugin tool's work, owned so it can outlive the call that started it.
pub(crate) type ToolWork = Pin<Box<dyn Future<Output = rumu_core::Result<ToolOutput>> + Send>>;

/// How long a foreground call of a background-capable tool runs before it
/// moves to the background, unless its `timeout` argument says otherwise.
const FOREGROUND_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest `timeout` a foreground call may ask for, in seconds.
const FOREGROUND_TIMEOUT_MAX: f64 = 600.0;
/// How much of a moved call's output so far its result shows.
const MOVED_OUTPUT_CHARS: usize = 10_000;

/// The foreground limit of a background-capable tool's call: its `timeout`
/// argument in seconds (capped), else [`FOREGROUND_TIMEOUT`] when `default`.
pub(crate) fn foreground_limit(args: &Value, default: bool) -> Option<Duration> {
    match args.get("timeout").and_then(Value::as_f64) {
        Some(secs) if secs > 0.0 => Some(Duration::from_secs_f64(secs.min(FOREGROUND_TIMEOUT_MAX))),
        _ => default.then_some(FOREGROUND_TIMEOUT),
    }
}

/// A foreground call [`Kernel::run_foreground`] runs: what the task it may
/// become is made of.
pub(crate) struct Foreground {
    pub session: SessionId,
    pub branch: BranchId,
    pub call_id: String,
    pub name: String,
    pub summary: String,
    pub source: String,
    /// The call's progress sink, for a call that may move (a model's);
    /// `None` stops it at its limit instead.
    pub handoff: Option<Arc<Handoff>>,
}

/// The progress sink of a call that may move to the background: forwards to
/// the call's own sink and keeps the text, until [`Handoff::switch`] sends
/// the rest to its task.
pub(crate) struct Handoff(Mutex<HandoffState>);

enum HandoffState {
    Call { sink: ProgressSink, output: String },
    Task(TaskId),
}

impl Handoff {
    pub(crate) fn new(sink: ProgressSink) -> Arc<Self> {
        Arc::new(Self(Mutex::new(HandoffState::Call {
            sink,
            output: String::new(),
        })))
    }

    /// The sink to give the call's [`Cx`].
    pub(crate) fn sink(self: &Arc<Self>, kernel: &Arc<Kernel>) -> ProgressSink {
        let handoff = self.clone();
        let kernel = kernel.clone();
        Arc::new(move |update| match &mut *handoff.0.lock().unwrap() {
            HandoffState::Call { sink, output } => {
                if let Progress::Append(text) = &update {
                    output.push_str(text);
                    keep_tail(output, TASK_OUTPUT_LIMIT);
                }
                sink(update);
            }
            HandoffState::Task(id) => kernel.task_progress(id, update),
        })
    }

    /// Send the rest of the progress to task `id`, seeding its buffer with
    /// the output so far, which is returned. Holding the lock throughout
    /// keeps a chunk arriving meanwhile behind it.
    fn switch(&self, kernel: &Kernel, id: &TaskId) -> String {
        let mut state = self.0.lock().unwrap();
        let so_far = match &mut *state {
            HandoffState::Call { output, .. } => std::mem::take(output),
            HandoffState::Task(_) => String::new(),
        };
        if !so_far.is_empty() {
            kernel.task_progress(id, Progress::Append(so_far.clone()));
        }
        *state = HandoffState::Task(id.clone());
        so_far
    }
}

/// What a task's work returned.
pub(crate) enum TaskOutcome {
    Result(Result<ToolOutput>),
    /// With whatever partial result the work left (an agent's summary).
    Cancelled(String),
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

    /// Wake every waiter: a task's status or `post_done` changed, or a
    /// branch may have gone idle.
    pub(crate) fn signal_tasks(&self) {
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

        let name = name.to_owned();
        self.spawn_tool_work(
            id.clone(),
            token,
            Box::pin(async move { plugin.call_tool(&name, args, &cx).await }),
        );
        Ok(ToolOutput::ok(started(&id, &label)))
    }

    /// Run a started tool's work as task `id` until it ends or is cancelled.
    fn spawn_tool_work(self: &Arc<Self>, id: TaskId, token: CancellationToken, work: ToolWork) {
        let kernel = self.clone();
        tokio::spawn(async move {
            let outcome = tokio::select! {
                biased;
                _ = token.cancelled() => TaskOutcome::Cancelled(String::new()),
                r = work => TaskOutcome::Result(r.map_err(Error::from)),
            };
            kernel.finish_task(&id, outcome).await;
        });
    }

    /// Run a foreground call of a background-capable tool for at most
    /// `limit`. A model's call still running then moves to the background as
    /// a task, the same work continuing there; a plugin's call (`code`) is
    /// stopped. While the call waits on the user (`Cx::decide`) the clock
    /// stops for good: the decision has a timeout of its own. `None` when the
    /// turn is aborted first.
    pub(crate) async fn run_foreground(
        self: &Arc<Self>,
        mut work: ToolWork,
        mut limit: Option<Duration>,
        call: Foreground,
        token: &CancellationToken,
    ) -> Option<ToolOutput> {
        let failed =
            |e: rumu_core::Error| ToolOutput::err(format!("tool `{}` failed: {e}", call.name));
        let after = loop {
            let deadline = async {
                match limit {
                    Some(limit) => tokio::time::sleep(limit).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // Abort wins even if the tool future is also ready (an
                // internal call cancelled by the same token), and a call that
                // ends right at its deadline is done, not moved.
                biased;
                _ = token.cancelled() => return None,
                r = &mut work => return Some(r.unwrap_or_else(failed)),
                () = deadline => {}
            }
            if !self.awaits_decision(&call.branch, &call.call_id) {
                break limit.unwrap_or_default();
            }
            limit = None;
        };
        let secs = after.as_secs_f64();
        let kind = TaskKind::Tool {
            name: call.name.clone(),
            args_summary: call.summary.clone(),
        };
        let label = kind.label("");
        let Some(handoff) = call.handoff else {
            return Some(ToolOutput::err(format!(
                "`{label}` timed out after {secs}s"
            )));
        };
        let created = self.create_task(
            &call.session,
            &call.branch,
            &call.call_id,
            kind,
            call.summary,
            call.source,
        );
        let (id, task_token) = match created {
            Ok(v) => v,
            Err(e) => {
                return Some(ToolOutput::err(format!(
                    "`{label}` timed out after {secs}s and could not move to the background: {e}"
                )));
            }
        };
        let so_far = handoff.switch(self, &id);
        self.spawn_tool_work(id.clone(), task_token, work);
        let mut text = format!(
            "still running after {secs}s, so it moved to the background as task {id} ({label}) — {}",
            follow(&id)
        );
        if !so_far.is_empty() {
            text.push_str("\noutput so far:\n");
            text.push_str(&last_chars(&so_far, MOVED_OUTPUT_CHARS));
        }
        Some(ToolOutput::ok(text))
    }

    /// Is a decision of this call waiting on the user?
    fn awaits_decision(&self, branch: &BranchId, call_id: &str) -> bool {
        let inner = self.inner.lock().unwrap();
        inner
            .decisions
            .values()
            .any(|d| d.params["branch"] == json!(branch) && d.params["call_id"] == call_id)
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
        ask: crate::turn::ModelAsk,
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
                .run_agent(
                    &owner,
                    &fork_call,
                    None,
                    &task,
                    inherit,
                    ask,
                    &token,
                    Some(&task_id),
                )
                .await
            {
                Ok(AgentRun::Summary(text)) => TaskOutcome::Result(Ok(ToolOutput::ok(text))),
                Ok(AgentRun::Interrupted(text)) => TaskOutcome::Cancelled(text),
                Ok(AgentRun::Aborted) => TaskOutcome::Cancelled(String::new()),
                Ok(AgentRun::Refused(message)) => TaskOutcome::Result(Ok(ToolOutput::err(message))),
                Err(e) => TaskOutcome::Result(Ok(ToolOutput::err(format!("agent failed: {e}")))),
            };
            kernel.finish_task(&task_id, outcome).await;
        });

        Ok(ToolOutput::ok(started(&id, &label)))
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
                    quiet: false,
                },
            );
            (id, token, info)
        };
        self.save_task(&info);
        self.signal_tasks();
        self.push("task.started", json!({ "task": info }));
        Ok((id, token))
    }

    /// Record which branch a background agent got.
    pub(crate) fn set_task_child(&self, id: &TaskId, child: &BranchId) {
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
                TaskOutcome::Cancelled(text) => {
                    (TaskStatus::Cancelled, Content::text(text), vec![], false)
                }
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
            let wake = !entry.quiet;
            entry.quiet = false;
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
        self.save_task(&info);
        self.deliver_completion(&info, note, effects, &source, wake)
            .await;
    }

    /// Persist a task's state in its session's journal, so a restart can tell
    /// what it lost (see [`Kernel::recover_tasks`]). Its output buffer and
    /// result stay out: the notes it posted already carry them.
    fn save_task(&self, info: &TaskInfo) {
        if let Err(e) = self.store.append_task(info) {
            self.warn(format!("could not record task {}: {e}", info.id));
        }
    }

    // ---- restart ----

    /// Adopt what earlier runs left in the journals: a task that was running
    /// when its kernel stopped comes back as [`TaskStatus::Lost`] with the
    /// completion note it never got to post; the others come back as they
    /// ended. Their output buffers and effects are gone — only the notes they
    /// posted and this table remain.
    pub(crate) async fn recover_tasks(self: &Arc<Self>) {
        let sessions = match self.store.list_sessions() {
            Ok(v) => v,
            Err(e) => return self.warn(format!("could not recover tasks: {e}")),
        };
        for session in sessions {
            let tasks = match self.store.tasks_of(&session.id) {
                Ok(v) => v,
                Err(e) => {
                    self.warn(format!(
                        "could not recover the tasks of {}: {e}",
                        session.id
                    ));
                    continue;
                }
            };
            for mut info in tasks {
                if !info.status.is_running() {
                    self.adopt_task(info);
                    continue;
                }
                info.status = TaskStatus::Lost;
                info.ended_at = Some(now());
                info.is_error = true;
                let content = Content::text(LOST_TASK);
                info.result = Some(content.clone());
                self.save_task(&info);
                let note = completion_text(&info, &content);
                self.adopt_task(info.clone());
                self.push("task.done", json!({ "task": info }));
                // A note, not a wake: the branch may have work of its own as
                // soon as the user comes back.
                self.deliver_completion(&info, note, vec![], SYSTEM_SOURCE, false)
                    .await;
            }
        }
    }

    /// Put a task the journal remembers into the live table, ended and with
    /// no output: nothing can run or cancel it any more.
    fn adopt_task(&self, info: TaskInfo) {
        let mut inner = self.inner.lock().unwrap();
        inner.tasks.entry(info.id.clone()).or_insert(TaskEntry {
            info,
            output: String::new(),
            dropped: 0,
            source: SYSTEM_SOURCE.to_owned(),
            token: CancellationToken::new(),
            // Its completion is already in the transcript: never wait on it.
            post_done: true,
            quiet: false,
        });
        drop(inner);
        self.signal_tasks();
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
                post: Post {
                    effects,
                    wake,
                    paced: false,
                },
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

    /// Is `branch` done for now: no loop running, no wake waiting to start
    /// one, and no background sub-agent of it (or of a sub-agent below it)
    /// still running or yet to report? A background tool task does not count:
    /// it may never end (a dev server), and its completion wakes the branch
    /// later like any post.
    pub fn is_idle(&self, branch: &BranchId) -> Result<bool> {
        let subtree = self.branch_subtree(branch)?;
        let inner = self.inner.lock().unwrap();
        let loops = subtree.iter().any(|b| {
            inner.running.contains_key(b) || inner.wake_pending.get(b).is_some_and(|p| p.active)
        });
        let tasks = inner.tasks.values().any(|e| {
            matches!(e.info.kind, TaskKind::Agent { .. })
                && subtree.contains(&e.info.branch)
                && (e.info.status.is_running() || !e.post_done)
        });
        Ok(!loops && !tasks)
    }

    /// Wait until `branch` [is idle](Self::is_idle), then push `branch.idle`:
    /// a subscriber sees every push of the work it waited for before that one.
    pub async fn wait_idle(&self, branch: &BranchId) -> Result<()> {
        let mut changed = self.task_changed.subscribe();
        while !self.is_idle(branch)? {
            // The sender lives as long as the kernel.
            let _ = changed.changed().await;
        }
        self.push("branch.idle", json!({ "branch": branch }));
        Ok(())
    }

    /// Cancel every running task owned by `branch` or one of its sub-agent
    /// descendants, leaving the branch's own reply alone. Their completion
    /// notes are still written; they wake their owners like a `task.kill`
    /// (`task.kill_all`), or with `quiet` not at all ([`Kernel::abort`], a
    /// stopped sub-agent).
    pub fn abort_tasks(&self, branch: &BranchId, quiet: bool) {
        let subtree = self.branch_subtree(branch).unwrap_or_else(|_| {
            let mut set = std::collections::HashSet::new();
            set.insert(branch.clone());
            set
        });
        let mut inner = self.inner.lock().unwrap();
        for entry in inner.tasks.values_mut() {
            if subtree.contains(&entry.info.branch) && entry.info.status.is_running() {
                entry.quiet = quiet;
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

    /// `task.kill` from a call on `branch` (`Cx::kill_task`): the same
    /// cancellation as [`Kernel::kill_task`], for the branch's own tasks and
    /// its sub-agents' only.
    pub(crate) async fn kill_task_from(&self, branch: &BranchId, id: &TaskId) -> Result<TaskInfo> {
        let Some((_, info)) = self.task_token(id) else {
            return Err(Error::TaskNotFound { id: id.clone() });
        };
        if !self.owns_task(branch, &info.branch) {
            return Err(Error::TaskNotOwned { id: id.clone() });
        }
        self.kill_task(id).await
    }
}

/// The result of a call started as a task: its id, and how to follow it.
fn started(id: &TaskId, label: &str) -> String {
    format!("started task {id} ({label}) — {}", follow(id))
}

/// How to follow a running task.
fn follow(id: &TaskId) -> String {
    format!(
        "you will be notified when it finishes; `read {RUMU_WORKSPACE}:/tasks/{id}` shows how \
         it is doing, `invoke` `task.kill` on it stops it"
    )
}
/// Keep the last [`TASK_OUTPUT_LIMIT`] bytes, on a char boundary.
fn trim_buffer(entry: &mut TaskEntry) {
    let cut = keep_tail(&mut entry.output, TASK_OUTPUT_LIMIT);
    if cut > 0 {
        entry.dropped += cut as u64;
        entry.info.truncated = true;
    }
}

/// Drop the front of `text` down to its last `limit` bytes, on a char
/// boundary; returns how many bytes went.
fn keep_tail(text: &mut String, limit: usize) -> usize {
    if text.len() <= limit {
        return 0;
    }
    let mut cut = text.len() - limit;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    text.drain(..cut);
    cut
}

/// The last `n` characters of `text`, marked when earlier ones were cut.
fn last_chars(text: &str, n: usize) -> String {
    let total = text.chars().count();
    if total <= n {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(total - n).collect();
    format!("[… {} earlier chars …]\n{tail}", total - n)
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
