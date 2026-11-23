//! Kernel integration tests with a scripted fake LLM and a configurable test
//! plugin. Covers: tool-calling turns, hook block/rewrite on tool.pre, the
//! note index, agent (plain-reply summaries), safe-point
//! queueing, out-of-band posts/wakes, abort, steering and internal plugin
//! calls (caller isolation, denied tools, traces).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rumu_core::kind;
use rumu_core::{
    Attachment, BranchId, BranchStatus, CardMount, Content, Cx, Decision, Event, HookOutput,
    HookSpec, Inbox, Invoke, Manifest, Node, NodeBody, NodeId, PathState, Plugin, Post, Progress,
    SCRATCH_WORKSPACE, SessionId, SessionRoot, Tagged, TaskDone, TaskKind, TaskStatus, ToolCall,
    ToolOutput, ToolSpec, Workspace, WorkspaceTarget, WorkspaceUse, Workspaces,
};
use rumu_kernel::{Delta, Kernel, Llm, LlmError, LlmResponse, OnDelta, ScriptedLlm};
use rumu_store::Store;
use serde_json::{Value, json};

// ---------- hook output helpers ----------

fn pass() -> HookOutput {
    HookOutput::default()
}

fn inject(_key: Option<&str>, text: &str) -> HookOutput {
    HookOutput {
        effects: vec![Content::text(text)],
        decision: Decision::Pass,
    }
}

fn block(reason: &str) -> HookOutput {
    HookOutput {
        effects: vec![],
        decision: Decision::Block {
            reason: reason.into(),
        },
    }
}

fn rewrite(patch: Value) -> HookOutput {
    HookOutput {
        effects: vec![],
        decision: Decision::Rewrite { patch },
    }
}

// ---------- fake LLM ----------

/// Replays the script and resolves `$task` placeholders (task ids are random).
struct FakeLlm {
    inner: ScriptedLlm,
    titles: std::sync::Mutex<rumu_core::Titles>,
}

impl FakeLlm {
    fn new(scripted: Vec<LlmResponse>) -> Self {
        Self {
            inner: ScriptedLlm::strict(scripted),
            titles: Default::default(),
        }
    }

    fn projections(&self) -> Vec<Vec<Node>> {
        self.inner.projections()
    }
}

#[async_trait]
impl Llm for FakeLlm {
    async fn complete(
        &self,
        pick: rumu_core::ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        let mut resp = self
            .inner
            .complete(pick, system, tools, nodes, on_delta)
            .await?;
        fill_task_id(&mut resp, nodes);
        Ok(resp)
    }

    fn titles(&self) -> rumu_core::Titles {
        *self.titles.lock().unwrap()
    }
}

/// Resolve a `$task` tool argument to the id of the task whose "started task
/// <id>" result is already in the projection (task ids are random).
fn fill_task_id(resp: &mut LlmResponse, nodes: &[Node]) {
    let id = nodes.iter().rev().find_map(|n| match &n.body {
        NodeBody::ToolResult { content, .. } => content
            .text
            .split_once("started task ")
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .map(str::to_owned),
        _ => None,
    });
    let Some(id) = id else { return };
    for call in &mut resp.tool_calls {
        if call.args.get("id").and_then(Value::as_str) == Some("$task") {
            call.args["id"] = json!(id);
        }
    }
}

// ---------- concurrency-tracking fake LLM ----------

/// Like [`FakeLlm`], but records how many sub-agent completions overlap.
/// A sub-agent call is recognised by its `You are now a sub-agent` user node
/// (the `context: "none"` children used by the concurrency tests).
struct TrackedLlm {
    inner: ScriptedLlm,
    active: AtomicUsize,
    max_active: AtomicUsize,
    child_sleep: Duration,
}

impl TrackedLlm {
    fn new(scripted: Vec<LlmResponse>, child_sleep: Duration) -> Self {
        Self {
            inner: ScriptedLlm::strict(scripted),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            child_sleep,
        }
    }
}

#[async_trait]
impl Llm for TrackedLlm {
    async fn complete(
        &self,
        pick: rumu_core::ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        let is_child = nodes.iter().any(|n| match &n.body {
            NodeBody::User { content } => content.text.contains("You are now a sub-agent"),
            _ => false,
        });
        if is_child {
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.child_sleep).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
        }
        self.inner
            .complete(pick, system, tools, nodes, on_delta)
            .await
    }
}

// ---------- failing fake LLM ----------

/// A genai-shaped transient failure (its `HttpError` Display).
const TRANSIENT_503: &str = "HTTP error.\nStatus: 503 Service Unavailable\nBody:\noverloaded";
/// The same shape with a caller-fault status.
const PERMANENT_400: &str = "HTTP error.\nStatus: 400 Bad Request\nBody:\nbad request";

/// Like [`FakeLlm`], but its first `fail` completions fail with `message`
/// (after streaming `leak` characters, to cover the
/// never-retry-after-a-delta rule). The failing budget is shared with the
/// test so it can be refilled.
struct FlakyLlm {
    script: Mutex<VecDeque<LlmResponse>>,
    seen: Mutex<Vec<Vec<Node>>>,
    fail: Arc<Mutex<usize>>,
    message: String,
    leak: String,
}

impl FlakyLlm {
    fn new(script: Vec<LlmResponse>, fail: usize, message: &str) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            seen: Mutex::new(vec![]),
            fail: Arc::new(Mutex::new(fail)),
            message: message.to_owned(),
            leak: String::new(),
        }
    }

    /// Text streamed before a failing attempt.
    fn leaking(mut self, leak: &str) -> Self {
        self.leak = leak.to_owned();
        self
    }

    fn projections(&self) -> Vec<Vec<Node>> {
        self.seen.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl Llm for FlakyLlm {
    async fn complete(
        &self,
        _pick: rumu_core::ModelPick<'_>,
        _system: &str,
        _tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError> {
        self.seen.lock().unwrap().push(nodes.to_vec());
        let failing = {
            let mut fail = self.fail.lock().unwrap();
            if *fail > 0 {
                *fail -= 1;
                true
            } else {
                false
            }
        };
        if failing {
            if let Some(cb) = &on_delta
                && !self.leak.is_empty()
            {
                cb(Delta::Text(self.leak.clone()));
            }
            return Err(LlmError::Request(self.message.clone()));
        }
        Ok(self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .expect("flaky LLM exhausted its script"))
    }
}

// ---------- test plugin ----------

type HookFn = Arc<dyn Fn(&str, &Event) -> HookOutput + Send + Sync>;
type ToolFn = Arc<dyn Fn(&str, Value, &Cx) -> ToolOutput + Send + Sync>;
type PushRx = tokio::sync::mpsc::UnboundedReceiver<Value>;

struct TestPlugin {
    manifest: Manifest,
    hook_fn: HookFn,
    tool_fn: ToolFn,
    calls: Mutex<Vec<(String, Value)>>,
    inbox: Mutex<Option<Arc<dyn Inbox>>>,
    /// Artificial hook latency (fail-closed test).
    delay: Duration,
}

impl TestPlugin {
    fn new(
        name: &str,
        tools: Vec<ToolSpec>,
        hooks: Vec<HookSpec>,
        hook_fn: HookFn,
        tool_fn: ToolFn,
    ) -> Self {
        Self {
            manifest: Manifest {
                name: name.to_owned(),
                instructions: Some(format!("You have the {name} plugin.")),
                tools,
                hooks,
            },
            hook_fn,
            tool_fn,
            calls: Mutex::new(vec![]),
            inbox: Mutex::new(None),
            delay: Duration::ZERO,
        }
    }

    fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    fn inbox(&self) -> Arc<dyn Inbox> {
        self.inbox
            .lock()
            .unwrap()
            .clone()
            .expect("the kernel attached an inbox")
    }

    fn post(&self, branch: &BranchId, post: Post) -> rumu_core::Result<()> {
        self.inbox().post(branch, post)
    }
}

#[async_trait]
impl Plugin for TestPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn attach(&self, inbox: Arc<dyn Inbox>) {
        *self.inbox.lock().unwrap() = Some(inbox);
    }

    async fn on_hook(
        &self,
        hook_id: &str,
        event: &Event,
        _cx: &Cx,
    ) -> rumu_core::Result<HookOutput> {
        if !self.delay.is_zero() {
            tokio::time::sleep(self.delay).await;
        }
        Ok((self.hook_fn)(hook_id, event))
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_owned(), args.clone()));
        // Special cases used by the injection / abort / code tests.
        match name {
            "append_marker" => {
                // Emulates a plugin recording a marker from a tool.
                let tag = args
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("card.use")
                    .to_owned();
                let data = args.get("data").cloned().unwrap_or(json!({}));
                return Ok(ToolOutput::ok("recorded").with_effect(Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(Tagged { tag, v: 1, data }),
                }));
            }
            "kill" => {
                // `task.kill` as the workspace plugin runs it.
                let id = rumu_core::TaskId::from(args["id"].as_str().unwrap_or_default());
                return Ok(match cx.kill_task(&id).await {
                    Ok(info) => ToolOutput::ok(format!("task {id}: {}", info.status.label())),
                    Err(e) => ToolOutput::err(e.to_string()),
                });
            }
            "probe_state" => {
                // Reports the default workspace as plugins see it mid-batch.
                let state = cx.state()?;
                return Ok(ToolOutput::ok(
                    state
                        .default_workspace
                        .clone()
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            "probe_index" => {
                // Counts structured payloads of one tag on the branch view.
                let tag = args.get("tag").and_then(Value::as_str).unwrap_or_default();
                let count = cx.index()?.by_tag(tag).count();
                return Ok(ToolOutput::ok(count.to_string()));
            }
            "sleep" => {
                tokio::time::sleep(Duration::from_secs(120)).await;
                return Ok(ToolOutput::ok("slept"));
            }
            // A background-capable tool: emits progress, waits, returns.
            "bgwork" => {
                let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(100);
                cx.progress(Progress::Append("line one\n".into()));
                cx.progress(Progress::Append("line two\n".into()));
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let out = args
                    .get("out")
                    .and_then(Value::as_str)
                    .unwrap_or("finished work")
                    .to_owned();
                return Ok(ToolOutput::ok(out));
            }
            "nap" => {
                let ms = args.get("ms").and_then(Value::as_u64).unwrap_or(20);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                return Ok(ToolOutput::ok("napped"));
            }
            // Invokes itself through Cx until the kernel's depth limit trips.
            "recurse" => {
                return match cx.invoke("recurse", json!({})).await {
                    Ok(r) => Ok(ToolOutput {
                        content: r.output.content,
                        is_error: r.output.is_error,
                        ..Default::default()
                    }),
                    Err(e) => Ok(ToolOutput::err(format!("{e}"))),
                };
            }
            // Tries an internal call; used to check the background-task
            // restriction (a background task must not invoke).
            "try_invoke" => {
                return match cx.invoke("plus", json!({ "a": 1, "b": 2 })).await {
                    Ok(r) => Ok(r.output),
                    Err(e) => Ok(ToolOutput::err(format!("{e}"))),
                };
            }
            // Emulates the `code` plugin: run a preset list of internal tool
            // calls through `Cx::invoke`, strictly in order.
            "script" => {
                let calls = args
                    .get("calls")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut results = Vec::new();
                for c in calls {
                    let name = c
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let inner_args = c.get("args").cloned().unwrap_or_else(|| json!({}));
                    match cx.invoke(&name, inner_args).await {
                        Ok(r) => results.push(json!({
                            "name": name,
                            "ok": !r.output.is_error,
                            "blocked": r.blocked,
                            "len": r.output.content.text.chars().count(),
                            "text": r.output.content.text.chars().take(200).collect::<String>(),
                            "dur_ms": r.dur_ms,
                        })),
                        Err(e) => results.push(json!({
                            "name": name,
                            "ok": false,
                            "error": format!("{e}"),
                        })),
                    }
                }
                return Ok(ToolOutput::ok(serde_json::to_string(&results).unwrap()));
            }
            "set_ws" => {
                let name = args
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                return Ok(
                    ToolOutput::ok("set").with_effect(Content::marker(&WorkspaceUse { name }))
                );
            }
            // Emulates the `code` plugin's agent fan-out: each agent call runs
            // in a detached task (so an abort cancels it through its parent
            // token instead of dropping the future mid-flight).
            "fanout" => {
                let tasks = args
                    .get("tasks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut handles = Vec::new();
                for t in tasks {
                    let task = t.as_str().unwrap_or_default().to_owned();
                    let cx = cx.clone();
                    handles.push(tokio::spawn(async move {
                        match cx
                            .invoke("agent", json!({ "task": task, "mode": "fresh" }))
                            .await
                        {
                            Ok(r) => r.output.content.text,
                            Err(e) => format!("{e}"),
                        }
                    }));
                }
                let mut texts = Vec::new();
                for h in handles {
                    texts.push(h.await.unwrap_or_default());
                }
                return Ok(ToolOutput::ok(texts.join("|")));
            }
            "big" => return Ok(ToolOutput::ok("x".repeat(50_000))),
            // Starts a long internal call, then cancels it without awaiting
            // (a `code` script that ended early).
            "orphan" => {
                let inner = cx.clone();
                let h =
                    tokio::spawn(async move { inner.invoke("nap", json!({ "ms": 30_000 })).await });
                tokio::time::sleep(Duration::from_millis(50)).await;
                cx.cancel_invokes();
                let r = tokio::time::timeout(Duration::from_secs(5), h).await;
                let text = match r {
                    Ok(Ok(Err(e))) => format!("cancelled: {e}"),
                    Ok(Ok(Ok(r))) => format!("finished: {}", r.output.content.text),
                    _ => "hung".into(),
                };
                return Ok(ToolOutput::ok(text));
            }
            "append_note" => {
                return Ok(ToolOutput::ok("noted").with_effect(Content::text("direct note")));
            }
            _ => {}
        }
        Ok((self.tool_fn)(name, args, cx))
    }
}

// ---------- fixtures ----------

fn tool_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_owned(),
        description: format!("test tool {name}"),
        schema: json!({ "type": "object", "properties": {} }),
        background: false,
        output_schema: None,
        parallel: false,
    }
}

fn hook(id: &str, matcher: Value) -> HookSpec {
    HookSpec {
        id: id.to_owned(),
        matcher,
        priority: 0,
    }
}

fn hook_prio(id: &str, matcher: Value, priority: i32) -> HookSpec {
    HookSpec {
        priority,
        ..hook(id, matcher)
    }
}

fn noop_hooks() -> HookFn {
    Arc::new(|_, _| pass())
}

fn noop_tools() -> ToolFn {
    Arc::new(|_, _, _| ToolOutput::ok("ok"))
}

/// The standard 2+3 adder used by most tool tests.
fn plus_tool() -> ToolFn {
    Arc::new(|_name, args, _| {
        let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
        ToolOutput::ok((a + b).to_string())
    })
}

fn block_hook(reason: &str) -> HookFn {
    let reason = reason.to_owned();
    Arc::new(move |_, _| block(&reason))
}

/// A plugin that exposes `tools`, runs `hooks`, and blocks with `reason`.
fn guard(tools: &[&str], hooks: Vec<HookSpec>, reason: &str) -> TestPlugin {
    plugin_with("guard", tools, hooks, block_hook(reason), noop_tools())
}

fn plugin_with(
    name: &str,
    tools: &[&str],
    hooks: Vec<HookSpec>,
    hook_fn: HookFn,
    tool_fn: ToolFn,
) -> TestPlugin {
    TestPlugin::new(
        name,
        tools.iter().map(|t| tool_spec(t)).collect(),
        hooks,
        hook_fn,
        tool_fn,
    )
}

/// Like [`plugin_with`], but every tool is marked `background: true`.
fn background_plugin(
    name: &str,
    tools: &[&str],
    hooks: Vec<HookSpec>,
    hook_fn: HookFn,
    tool_fn: ToolFn,
) -> TestPlugin {
    TestPlugin::new(
        name,
        tools
            .iter()
            .map(|t| ToolSpec {
                background: true,
                output_schema: None,
                parallel: false,
                ..tool_spec(t)
            })
            .collect(),
        hooks,
        hook_fn,
        tool_fn,
    )
}

/// A plugin that also exposes the `script` tool (internal code calls).
fn script_host(
    extra_tools: &[&str],
    hooks: Vec<HookSpec>,
    hook_fn: HookFn,
    tool_fn: ToolFn,
) -> TestPlugin {
    let mut tools = vec!["script"];
    tools.extend_from_slice(extra_tools);
    plugin_with("script-host", &tools, hooks, hook_fn, tool_fn)
}

fn kernel_with(
    plugins: Vec<Arc<dyn Plugin>>,
    scripted: Vec<LlmResponse>,
    workspaces: Workspaces,
) -> (Arc<Kernel>, Arc<FakeLlm>) {
    let store = Store::in_memory();
    let llm = Arc::new(FakeLlm::new(scripted));
    let kernel = Kernel::new(Arc::new(store), llm.clone(), plugins, workspaces).unwrap();
    (kernel, llm)
}

/// A kernel over `workspaces` and a session outside any project. Every
/// registered workspace is addressable from it; one of them may still be the
/// branch default through a `workspace.use` marker.
async fn setup_with_workspaces(
    plugins: Vec<Arc<dyn Plugin>>,
    scripted: Vec<LlmResponse>,
    workspaces: Workspaces,
) -> (Arc<Kernel>, Arc<FakeLlm>, SessionId, BranchId) {
    let (kernel, llm) = kernel_with(plugins, scripted, workspaces);
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    (
        kernel,
        llm,
        session.id.clone(),
        BranchId::from(session.main_branch.0.as_str()),
    )
}

async fn setup(
    plugins: Vec<Arc<dyn Plugin>>,
    scripted: Vec<LlmResponse>,
) -> (Arc<Kernel>, Arc<FakeLlm>, SessionId, BranchId) {
    setup_with_workspaces(plugins, scripted, Workspaces::default()).await
}

fn project(name: &str, root: &str) -> Workspaces {
    Workspaces::new(vec![Workspace {
        name: name.into(),
        ssh: None,
        root: root.into(),
        read_only: false,
    }])
}

/// A registry of `workspaces` (`name`, `root`) in which the project deck
/// `deck` mounts `mounts`, in order.
fn registry_with(workspaces: &[(&str, &str)], deck: &str, mounts: &[&str]) -> Workspaces {
    let registry = Workspaces::new(
        workspaces
            .iter()
            .map(|(name, root)| Workspace {
                name: (*name).into(),
                ssh: None,
                root: (*root).into(),
                read_only: false,
            })
            .collect(),
    );
    registry
        .update_both(|_, projects| {
            projects.insert(deck.into(), mounts.iter().map(|n| (*n).into()).collect());
            Ok::<_, ()>(())
        })
        .unwrap();
    registry
}

fn project_mount(deck: &str) -> CardMount {
    CardMount::new(deck, format!("/cards/{deck}"), true)
}

/// The notes `session.move` writes to move a session into project `deck`
/// (out of its project with `None`), from `from`.
fn move_notes(from: Option<&str>, deck: Option<&str>, text: &str) -> Vec<Content> {
    let gone = from.map(|from| {
        Content::note_with(
            "left",
            &rumu_core::CardUnmount {
                at: format!("/cards/{from}"),
            },
        )
    });
    let added = deck.map(|deck| Content::note_with(text, &project_mount(deck)));
    gone.into_iter().chain(added).collect()
}

/// A kernel over `registry` and a session that starts in project `deck`.
async fn setup_in_project(
    plugins: Vec<Arc<dyn Plugin>>,
    scripted: Vec<LlmResponse>,
    registry: Workspaces,
    deck: &str,
) -> (Arc<Kernel>, Arc<FakeLlm>, SessionId, BranchId) {
    let (kernel, llm) = kernel_with(plugins, scripted, registry);
    let (session, _) = kernel
        .create_rooted_session(SessionRoot {
            assistant: "assistant".into(),
            mounts: vec![project_mount(deck)],
        })
        .await
        .unwrap();
    (kernel, llm, session.id.clone(), session.main_branch.clone())
}

/// Drive pushes until `turn.done`/`turn.error` for `branch` (or any branch
/// when `branch` is None).
async fn wait_turn(rx: &mut PushRx, branch: Option<&str>) -> Value {
    while let Some(v) = rx.recv().await {
        let m = v["method"].as_str().unwrap_or_default();
        if (m == "turn.done" || m == "turn.error")
            && branch.is_none_or(|b| v["params"]["branch"].as_str() == Some(b))
        {
            return v;
        }
    }
    panic!("push stream ended before turn finished");
}

/// Drive pushes until `turn.done`/`turn.error`, collecting every push seen
/// along the way (for assertions on `node.appended`).
async fn wait_turn_collect(rx: &mut PushRx, branch: Option<&str>) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    while let Some(v) = rx.recv().await {
        let m = v["method"].as_str().unwrap_or_default();
        if (m == "turn.done" || m == "turn.error")
            && branch.is_none_or(|b| v["params"]["branch"].as_str() == Some(b))
        {
            return (v, seen);
        }
        seen.push(v);
    }
    panic!("push stream ended before turn finished");
}

/// Prompt, wait for the turn, and return `(done, root→head nodes, state)`.
async fn turn_on(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    rx: &mut PushRx,
    text: &str,
    filter: Option<&str>,
) -> (Value, Vec<Node>, PathState) {
    kernel.prompt(branch, text).await.unwrap();
    let done = wait_turn(rx, filter).await;
    let (nodes, state) = kernel.branch_path(branch).unwrap();
    (done, nodes, state)
}

async fn turn(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    rx: &mut PushRx,
    text: &str,
) -> (Value, Vec<Node>, PathState) {
    turn_on(kernel, branch, rx, text, None).await
}

/// Single-turn convenience: tap, prompt, wait, load.
async fn run_turn(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    text: &str,
) -> (Value, Vec<Node>, PathState) {
    let mut rx = kernel.tap_pushes();
    turn(kernel, branch, &mut rx, text).await
}

/// Same as [`run_turn`], but only waits for `branch`'s own turn (agents).
async fn run_turn_on(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    text: &str,
) -> (Value, Vec<Node>, PathState) {
    let mut rx = kernel.tap_pushes();
    turn_on(kernel, branch, &mut rx, text, Some(branch.0.as_str())).await
}

async fn turn_collect(
    kernel: &Arc<Kernel>,
    branch: &BranchId,
    rx: &mut PushRx,
    text: &str,
) -> (Value, Vec<Value>) {
    kernel.prompt(branch, text).await.unwrap();
    wait_turn_collect(rx, None).await
}

fn body_kinds(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .map(|n| match &n.body {
            NodeBody::User { .. } => "user".to_owned(),
            NodeBody::Assistant { tool_calls, .. } if tool_calls.is_empty() => {
                "assistant".to_owned()
            }
            NodeBody::Assistant { .. } => "assistant+calls".to_owned(),
            NodeBody::ToolResult { .. } => "tool_result".to_owned(),
            NodeBody::Note { content, .. } => match content.data.as_ref() {
                Some(tagged) if content.text.is_empty() => format!("marker[{}]", tagged.tag),
                _ => "note".to_owned(),
            },
        })
        .collect()
}

fn result_text(nodes: &[Node], call_id: &str) -> (String, bool) {
    for n in nodes {
        if let NodeBody::ToolResult {
            call_id: c,
            content,
            is_error,
            ..
        } = &n.body
            && c == call_id
        {
            return (content.text.clone(), *is_error);
        }
    }
    panic!("no tool result for {call_id}");
}

// ---------- turns ----------

/// Titled by the latest prompt, the title follows each one; by the first,
/// it stays.
#[tokio::test]
async fn titles_follow_the_first_or_the_latest_prompt() {
    let (kernel, llm, _s, b) = setup(
        vec![],
        vec![
            LlmResponse::text("a"),
            LlmResponse::text("b"),
            LlmResponse::text("c"),
        ],
    )
    .await;
    let title = || kernel.list_sessions().unwrap().remove(0).title;
    run_turn(&kernel, &b, "first").await;
    run_turn(&kernel, &b, "second").await;
    assert_eq!(title(), "first");
    *llm.titles.lock().unwrap() = rumu_core::Titles::Last;
    run_turn(&kernel, &b, "third").await;
    assert_eq!(title(), "third");
}

/// With [`Titles::Last`] every prompt retitles the session — but a
/// sub-agent's task is a User node on the child branch, not one of the
/// session's own prompts, so the parent's prompt keeps the title.
#[tokio::test]
async fn sub_agent_task_does_not_retitle_the_session() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        Arc::new(|_, _, _| ToolOutput::ok("")),
    ));
    let (kernel, llm, _session, b) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({"mode": "fork", "background": false, "task": "find the config file"}),
            )]),
            // the sub-agent ends with a plain reply
            LlmResponse::text("config lives in /etc/rumu"),
            // the parent finishes
            LlmResponse::text("all done"),
        ],
    )
    .await;
    *llm.titles.lock().unwrap() = rumu_core::Titles::Last;
    run_turn(&kernel, &b, "look around").await;
    assert_eq!(
        kernel.list_sessions().unwrap().remove(0).title,
        "look around"
    );
}

/// A title is written anew on request, only while a model writes them.
#[tokio::test]
async fn retitle_asks_the_title_model() {
    let (kernel, llm, session, b) = setup(
        vec![],
        vec![
            LlmResponse::text("the sidebar has no rename entry"),
            LlmResponse::text("and then some"),
            LlmResponse::text("\"Greetings.\""),
        ],
    )
    .await;
    run_turn(&kernel, &b, "hello there").await;
    run_turn(&kernel, &b, "what next").await;
    assert!(matches!(
        kernel.retitle(&session).await,
        Err(rumu_kernel::Error::NoTitleModel)
    ));
    *llm.titles.lock().unwrap() = rumu_core::Titles::Model;
    kernel.retitle(&session).await.unwrap();
    assert_eq!(kernel.list_sessions().unwrap().remove(0).title, "Greetings");
    // It reads the first prompt, the first reply and the latest prompt.
    let asked = llm.projections().pop().unwrap();
    let [
        Node {
            body: NodeBody::User { content },
            ..
        },
    ] = asked.as_slice()
    else {
        panic!("the title model is asked with one user message: {asked:?}");
    };
    for part in [
        "hello there",
        "the sidebar has no rename entry",
        "what next",
    ] {
        assert!(
            content.text.contains(part),
            "{part:?} in {:?}",
            content.text
        );
    }
}

/// A title set by hand lands as given; a blank one leaves it alone.
#[tokio::test]
async fn a_hand_written_title_lands_as_given() {
    let (kernel, _llm, session, b) = setup(vec![], vec![LlmResponse::text("a")]).await;
    run_turn(&kernel, &b, "hello there").await;
    kernel.set_title(&session, "  手写标题  ").unwrap();
    assert_eq!(kernel.list_sessions().unwrap().remove(0).title, "手写标题");
    kernel.set_title(&session, "   ").unwrap();
    assert_eq!(kernel.list_sessions().unwrap().remove(0).title, "手写标题");
}

#[tokio::test]
async fn tool_calling_turn() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["plus"],
        vec![],
        noop_hooks(),
        plus_tool(),
    ));
    let (kernel, llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "plus", json!({"a": 2, "b": 3}))]),
            LlmResponse::text("sum is 5"),
        ],
    )
    .await;

    let (done, nodes, _state) = run_turn(&kernel, &branch, "add 2 and 3").await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "assistant+calls",
            "tool_result",
            "assistant"
        ]
    );
    assert_eq!(result_text(&nodes, "c1"), ("5".to_owned(), false));

    // The tool result was in context for the second LLM call.
    let projections = llm.projections();
    assert_eq!(projections.len(), 2);
    assert_eq!(body_kinds(&projections[1])[3], "tool_result");

    // Tools frozen + sorted in what the LLM receives (the kernel's built-in
    // `agent` plus the plugin's).
    assert_eq!(
        kernel
            .tools()
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["agent", "plus"]
    );
    assert!(kernel.system_prompt().contains("You have the calc plugin."));
}

#[tokio::test]
async fn hook_actions_on_tool_pre() {
    let plugin = Arc::new(guard(
        &["plus"],
        vec![hook(
            "no-math",
            json!({ "event": kind::TOOL_PRE, "tool": "plus" }),
        )],
        "no math today",
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "plus", json!({}))]),
            LlmResponse::text("fine"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "add something").await;
    assert_eq!(
        result_text(&nodes, "c1"),
        ("Blocked by hook: no math today".to_owned(), true)
    );
}

#[tokio::test]
async fn hook_blocks_prompt() {
    let plugin = Arc::new(guard(
        &[],
        vec![hook("no-prompts", json!({ "event": kind::PROMPT }))],
        "not in the mood",
    ));
    let (kernel, _llm, _s, branch) = setup(vec![plugin], vec![]).await;

    let (done, nodes, _) = run_turn(&kernel, &branch, "hello?").await;
    assert_eq!(done["method"], "turn.error");
    assert_eq!(done["params"]["message"], "not in the mood");
    assert_eq!(body_kinds(&nodes), vec!["marker[session.root]"]); // nothing appended
}

#[tokio::test]
async fn tool_error_results() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["plus"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "does-not-exist", json!({}))]),
            LlmResponse::text("ok"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "call a ghost").await;
    let (text, err) = result_text(&nodes, "c1");
    assert!(err);
    assert!(text.contains("no plugin provides tool `does-not-exist`"));
    // And the turn carried on to a final assistant message.
    assert_eq!(body_kinds(&nodes).last().unwrap(), "assistant");
}

/// Every append announces the branch with the head it now has. A client that
/// keeps the pushed copy of the branch replays `node.appended` (moves the
/// head) and `branch.updated` (replaces the branch); a copy holding the head
/// from before the append would put the head back one node, and since the
/// next append's parent no longer matches, the client would stop advancing
/// there — the turn's reply then reads as an interruption.
#[tokio::test]
async fn branch_updated_carries_the_new_head() {
    let (kernel, _llm, _s, b) =
        setup(vec![], vec![LlmResponse::text("a"), LlmResponse::text("b")]).await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&b, "hi").await.unwrap();
    let (done, seen) = wait_turn_collect(&mut rx, Some(b.0.as_str())).await;
    assert_eq!(done["method"], "turn.done");

    let mut head = String::new();
    for v in &seen {
        match v["method"].as_str() {
            Some("node.appended") => {
                head = v["params"]["node"]["id"].as_str().unwrap().to_owned();
            }
            Some("branch.updated") => {
                let branch = &v["params"]["branch"];
                if branch["id"].as_str() == Some(b.0.as_str()) {
                    head = branch["head"].as_str().unwrap().to_owned();
                }
            }
            _ => {}
        }
    }
    let (nodes, _) = kernel.branch_path(&b).unwrap();
    assert_eq!(
        head,
        nodes.last().unwrap().id.0,
        "client head lags the path"
    );
}

// ---------- image attachments ----------

fn attachment(hash: &str) -> Attachment {
    Attachment {
        kind: rumu_core::AttachmentKind::Image,
        hash: hash.to_owned(),
        mime: "image/png".into(),
        name: Some(format!("{hash}.png")),
        width: 4,
        height: 3,
        size: 10,
    }
}

/// The first user or tool-result content matching `f`.
fn content_where(nodes: &[Node], f: impl Fn(&Content) -> bool) -> &Content {
    nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::User { content } | NodeBody::ToolResult { content, .. } if f(content) => {
                Some(content)
            }
            _ => None,
        })
        .expect("a matching content")
}

/// Attachments travel the whole path: the prompt event shows them to hooks (a
/// text rewrite keeps them), the user node carries them, a tool's output
/// attachments survive into the ToolResult node, and the next LLM projection
/// sees both contents whole.
#[tokio::test]
async fn attachments_travel_with_prompts_and_tool_results() {
    let payloads = Arc::new(Mutex::new(vec![]));
    let recorder = payloads.clone();
    let rewriter = Arc::new(plugin_with(
        "guard",
        &[],
        vec![hook("rewrite", json!({ "event": kind::PROMPT }))],
        Arc::new(move |_, ev: &Event| {
            recorder.lock().unwrap().push(ev.payload.clone());
            rewrite(json!({ "text": "rewritten text" }))
        }),
        noop_tools(),
    ));
    let camera = Arc::new(plugin_with(
        "camera",
        &["snap"],
        vec![],
        noop_hooks(),
        Arc::new(|_, _, _| ToolOutput {
            content: Content::text("took [Image #1]").with_attachments([attachment("a1")]),
            ..Default::default()
        }),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![rewriter, camera],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "snap", json!({}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel
        .prompt(
            &branch,
            Content::text("see [Image #1]").with_attachments([attachment("b2")]),
        )
        .await
        .unwrap();
    let (done, _) = collect_turn(&mut rx, branch.0.as_str()).await;
    assert_eq!(done["method"], "turn.done");

    // The hook saw text and attachments; its rewrite changed only the text.
    {
        let payloads = payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1, "{payloads:?}");
        assert_eq!(payloads[0]["text"], "see [Image #1]");
        assert_eq!(payloads[0]["attachments"][0]["hash"], "b2");
    }
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let user = content_where(&nodes, |c| c.text == "rewritten text");
    assert_eq!(user.attachments, vec![attachment("b2")]);
    let result = content_where(&nodes, |c| c.text.starts_with("took"));
    assert_eq!(result.attachments, vec![attachment("a1")]);

    // The follow-up request projected both contents whole.
    let projections = llm.projections();
    let projected = content_where(&projections[1], |c| c.text.starts_with("took"));
    assert_eq!(projected.attachments, vec![attachment("a1")]);
}

/// `regenerate` replays the original prompt's attachments, not just its text:
/// the replayed prompt event shows hooks the same attachments again.
#[tokio::test]
async fn regenerate_replays_the_prompt_attachments() {
    let payloads = Arc::new(Mutex::new(vec![]));
    let recorder = payloads.clone();
    let plugin = Arc::new(plugin_with(
        "watcher",
        &[],
        vec![hook("watch", json!({ "event": kind::PROMPT }))],
        Arc::new(move |_, ev: &Event| {
            recorder.lock().unwrap().push(ev.payload.clone());
            pass()
        }),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![LlmResponse::text("first"), LlmResponse::text("again")],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel
        .prompt(
            &branch,
            Content::text("see [Image #1]").with_attachments([attachment("c3")]),
        )
        .await
        .unwrap();
    let _ = collect_turn(&mut rx, branch.0.as_str()).await;
    let prompt_node = kernel
        .branch_path(&branch)
        .unwrap()
        .0
        .into_iter()
        .find(|n| matches!(n.body, NodeBody::User { .. }))
        .expect("the turn appended a user node")
        .id;

    let regenerated = kernel
        .regenerate(&prompt_node, Default::default(), false)
        .await
        .unwrap();
    let _ = collect_turn(&mut rx, regenerated.id.0.as_str()).await;

    // Both prompt events (original + replay) carried the attachments.
    let payloads = payloads.lock().unwrap();
    assert_eq!(payloads.len(), 2, "{payloads:?}");
    for p in payloads.iter() {
        assert_eq!(p["attachments"][0]["hash"], "c3");
    }
}

// ---------- turn failures: retry, marker, manual retry ----------

/// A kernel over a [`FlakyLlm`] (most failure tests need their own script).
async fn flaky_setup(
    script: Vec<LlmResponse>,
    fail: usize,
    message: &str,
) -> (Arc<Kernel>, Arc<FlakyLlm>, BranchId) {
    let llm = Arc::new(FlakyLlm::new(script, fail, message));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    (kernel, llm, session.main_branch.clone())
}

/// A transient failure with nothing streamed yet is replayed: one
/// `turn.retrying` push, then the turn finishes normally — no marker.
#[tokio::test(start_paused = true)]
async fn transient_failure_is_retried_to_success() {
    let (kernel, llm, branch) =
        flaky_setup(vec![LlmResponse::text("recovered")], 1, TRANSIENT_503).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    let retrying: Vec<&Value> = seen
        .iter()
        .filter(|v| v["method"] == "turn.retrying")
        .collect();
    assert_eq!(retrying.len(), 1, "{seen:?}");
    assert_eq!(retrying[0]["params"]["branch"], branch.0);
    assert_eq!(retrying[0]["params"]["attempt"], 1);
    assert_eq!(retrying[0]["params"]["max"], rumu_kernel::MAX_LLM_RETRIES);
    let ms = retrying[0]["params"]["retry_in_ms"].as_u64().unwrap();
    assert!((600..=1400).contains(&ms), "jittered first backoff: {ms}");
    assert!(
        retrying[0]["params"]["message"]
            .as_str()
            .unwrap()
            .contains("503")
    );

    assert_eq!(llm.calls(), 2, "exactly one replay");
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&nodes),
        vec!["marker[session.root]", "user", "assistant"]
    );
    let NodeBody::Assistant { stats, .. } = &nodes[2].body else {
        unreachable!()
    };
    assert_eq!(stats.retries, 1, "the reply records its replay");
}

/// A transient failure that never clears exhausts the budget, and the turn is
/// recorded: marker at the head with the message, `turn.error` says recorded.
#[tokio::test(start_paused = true)]
async fn persistent_transient_failure_records_a_marker() {
    let (kernel, llm, branch) = flaky_setup(vec![], usize::MAX, TRANSIENT_503).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error", "{done}");
    assert_eq!(done["params"]["recorded"], true);
    assert!(done["params"]["message"].as_str().unwrap().contains("503"));

    let retries = seen
        .iter()
        .filter(|v| v["method"] == "turn.retrying")
        .count();
    assert_eq!(retries, rumu_kernel::MAX_LLM_RETRIES as usize);
    assert_eq!(llm.calls(), rumu_kernel::MAX_LLM_RETRIES as usize + 1);

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&nodes),
        vec!["marker[session.root]", "user", "marker[turn.error]"]
    );
    let marker = nodes.last().unwrap();
    let NodeBody::Note { content, .. } = &marker.body else {
        panic!("the marker is a note");
    };
    let payload = content
        .decode::<rumu_core::TurnError>()
        .expect("turn.error marker");
    assert!(payload.message.contains("503"), "{}", payload.message);
}

/// A caller-fault status is terminal on the first attempt: no retry, no
/// `turn.retrying`, marker written.
#[tokio::test(start_paused = true)]
async fn non_transient_failure_is_not_retried() {
    let (kernel, llm, branch) = flaky_setup(vec![], usize::MAX, PERMANENT_400).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error", "{done}");
    assert_eq!(done["params"]["recorded"], true);
    assert!(
        !seen.iter().any(|v| v["method"] == "turn.retrying"),
        "{seen:?}"
    );
    assert_eq!(llm.calls(), 1, "no replay");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(body_kinds(&nodes).last().unwrap(), "marker[turn.error]");
}

/// The session's status follows its main line: running while a turn runs,
/// failed once a turn ends in an error, idle again after a turn that ends
/// well.
#[tokio::test(start_paused = true)]
async fn session_status_follows_the_main_line() {
    use rumu_core::SessionStatus;
    let (kernel, _llm, branch) = flaky_setup(vec![LlmResponse::text("ok")], 1, PERMANENT_400).await;
    let mut rx = kernel.tap_pushes();
    let status = || kernel.list_sessions().unwrap()[0].status;
    assert_eq!(status(), SessionStatus::Idle);

    kernel.prompt(&branch, "hello").await.unwrap();
    assert_eq!(status(), SessionStatus::Running);
    wait_turn(&mut rx, Some(&branch.0)).await;
    assert_eq!(status(), SessionStatus::Failed);

    kernel.prompt(&branch, "again").await.unwrap();
    assert_eq!(status(), SessionStatus::Running);
    // The loop ends after `turn.done`, and says so.
    let session = loop {
        let v = rx.recv().await.expect("session.updated at the loop's end");
        if v["method"] == "session.updated" && v["params"]["session"].get("status").is_none() {
            break v;
        }
    };
    assert_eq!(status(), SessionStatus::Idle, "{session}");
}

/// Once a delta reached the UI, replaying is unsafe: even a transient
/// failure fails the turn on the first attempt (the leaked text stays a
/// transient push; the tree records only the marker).
#[tokio::test(start_paused = true)]
async fn failure_after_a_delta_is_not_retried() {
    let llm = Arc::new(FlakyLlm::new(vec![], usize::MAX, TRANSIENT_503).leaking("partial answer"));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error", "{done}");
    assert_eq!(done["params"]["recorded"], true);
    assert!(
        !seen.iter().any(|v| v["method"] == "turn.retrying"),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|v| { v["method"] == "llm.delta" && v["params"]["text"] == "partial answer" })
    );
    assert_eq!(llm.calls(), 1, "no replay");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(body_kinds(&nodes).last().unwrap(), "marker[turn.error]");
}

/// Aborting during a backoff wait ends the turn promptly, like any abort: no
/// further LLM call, no marker.
#[tokio::test(start_paused = true)]
async fn abort_during_backoff_ends_the_turn() {
    let (kernel, llm, branch) = flaky_setup(vec![], usize::MAX, TRANSIENT_503).await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    wait_push(&mut rx, |v| v["method"] == "turn.retrying", "retry wait").await;
    kernel.abort(&branch).unwrap();

    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");
    assert_eq!(llm.calls(), 1, "the aborted backoff never replays");
    assert!(!seen.iter().any(|v| v["method"] == "turn.error"));

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(body_kinds(&nodes), vec!["marker[session.root]", "user"]);
}

/// `branch.retry` runs the step loop again on the same branch: the marker
/// stays on the path (behind the new reply), no new user node, and the turn
/// announces itself with reason "retry".
#[tokio::test(start_paused = true)]
async fn retry_after_failure_appends_after_the_marker() {
    let (kernel, llm, branch) = flaky_setup(
        vec![LlmResponse::text("recovered")],
        usize::MAX,
        TRANSIENT_503,
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error");
    let first_calls = llm.calls();

    *llm.fail.lock().unwrap() = 0; // the provider recovered
    kernel.retry(&branch, Default::default()).await.unwrap();
    let started = wait_push(
        &mut rx,
        |v| v["method"] == "turn.started" && v["params"]["branch"] == branch.0,
        "retry turn started",
    )
    .await;
    assert_eq!(started["params"]["reason"], "retry");
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    assert_eq!(llm.calls(), first_calls + 1, "one more completion");
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "marker[turn.error]",
            "assistant"
        ]
    );
    // The marker is on the path the kernel hands the adapter, but the
    // adapter's projection skips it (empty-text note) — asserted in
    // rumu-llm. Here: the retry saw the prompt exactly once, no duplicates.
    let projections = llm.projections();
    let last = projections.last().unwrap();
    assert_eq!(
        last.iter()
            .filter(|n| matches!(&n.body, NodeBody::User { .. }))
            .count(),
        1
    );
}

/// A retry carries the composer's pick: a model chosen after the failure
/// is recorded before the loop runs again, so the retry uses it.
#[tokio::test(start_paused = true)]
async fn retry_applies_the_current_choice() {
    use rumu_core::ModelUse;
    let (kernel, llm, branch) = flaky_setup(
        vec![LlmResponse::text("recovered")],
        usize::MAX,
        TRANSIENT_503,
    )
    .await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error");

    *llm.fail.lock().unwrap() = 0;
    let cheap = ModelUse {
        model: "rumu/cheap".into(),
        effort: None,
    };
    let choice = rumu_kernel::Choice {
        model: Some(Some(cheap.clone())),
    };
    kernel.retry(&branch, choice).await.unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    let (nodes, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(st.model, Some(cheap));
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "marker[turn.error]",
            "marker[model.use]",
            "assistant"
        ]
    );
}

/// `retry` is refused when the branch already has its reply, and while a
/// turn is running.
#[tokio::test]
async fn retry_refused_when_answered_or_busy() {
    let (kernel, _llm, _s, branch) = setup(vec![], vec![LlmResponse::text("hi")]).await;
    run_turn(&kernel, &branch, "hello").await;
    let err = kernel.retry(&branch, Default::default()).await.unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::NothingToRetry), "{err:#}");

    // Busy with the user message still unanswered: TurnBusy.
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let llm = Arc::new(GatedLlm {
        started: started_tx,
        release: tokio::sync::Semaphore::new(0),
        inner: ScriptedLlm::strict(vec![LlmResponse::text("hi")]),
    });
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    kernel.prompt(&branch, "go").await.unwrap();
    started_rx.recv().await.expect("first LLM call in flight");
    let err = kernel.retry(&branch, Default::default()).await.unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::TurnBusy), "{err:#}");
    llm.release.add_permits(1);
    let mut rx = kernel.tap_pushes();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
}

/// A prompt hook's import leaves a text note between the user node and the
/// failure marker (flushed before the LLM call). Notes never answer a
/// prompt, so the branch still owes its reply and retry works.
#[tokio::test(start_paused = true)]
async fn retry_works_behind_a_hook_note() {
    let llm = Arc::new(FlakyLlm::new(
        vec![LlmResponse::text("recovered")],
        usize::MAX,
        TRANSIENT_503,
    ));
    let importer = Arc::new(plugin_with(
        "memory",
        &[],
        vec![hook("import", json!({ "event": kind::PROMPT }))],
        Arc::new(|_, _| inject(Some("card:x@a1b2c3d4"), "# x\nimported body")),
        noop_tools(),
    ));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![importer],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "hello").await.unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.error");
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&nodes),
        vec!["marker[session.root]", "user", "note", "marker[turn.error]"]
    );

    *llm.fail.lock().unwrap() = 0; // the provider recovered
    kernel.retry(&branch, Default::default()).await.unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "note",
            "marker[turn.error]",
            "assistant"
        ]
    );
}

/// A prompt-hook refusal is not a recorded failure: the regenerate's
/// `turn.error` marks itself `blocked`, nothing lands on the branch, and
/// `retry` — which would replay the loop without dispatching the prompt
/// event — is refused, so the hook's veto cannot be bypassed.
#[tokio::test]
async fn retry_does_not_bypass_a_refused_prompt() {
    let refusals = Arc::new(AtomicUsize::new(0));
    let seen_prompts = refusals.clone();
    let plugin = Arc::new(plugin_with(
        "guard",
        &[],
        vec![hook("one-prompt", json!({ "event": kind::PROMPT }))],
        Arc::new(move |_, _| {
            // The first prompt passes; the regenerate's replay is refused.
            if seen_prompts.fetch_add(1, Ordering::SeqCst) == 0 {
                pass()
            } else {
                block("not again")
            }
        }),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(vec![plugin], vec![LlmResponse::text("first")]).await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "hello").await;
    let prompt = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::User { .. }))
        .expect("the turn appended a user node")
        .id
        .clone();
    let calls_after_first = llm.projections().len();

    let mut rx = kernel.tap_pushes();
    let regenerated = kernel
        .regenerate(&prompt, Default::default(), false)
        .await
        .unwrap();
    let (done, _) = collect_turn(&mut rx, regenerated.id.0.as_str()).await;
    assert_eq!(done["method"], "turn.error");
    assert_eq!(done["params"]["blocked"], true, "{done}");
    assert_eq!(done["params"]["message"], "not again");

    // The forked branch holds only the inherited path: no marker, and the
    // user node at the head owes a reply — yet there is no recorded failure
    // to replay.
    let (nodes, _) = kernel.branch_path(&regenerated.id).unwrap();
    assert_eq!(body_kinds(&nodes), vec!["marker[session.root]", "user"]);
    let err = kernel
        .retry(&regenerated.id, Default::default())
        .await
        .unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::NothingToRetry), "{err:#}");
    assert_eq!(llm.projections().len(), calls_after_first, "no LLM call");
}

// ---------- tool progress ----------

/// A tool's `cx.progress` updates reach clients as `tool.progress` pushes,
/// keyed by branch + call id; `Append` and `Data` keep their shapes.
#[tokio::test]
async fn tool_progress_is_pushed() {
    let plugin = Arc::new(plugin_with(
        "prog",
        &["slowrun"],
        vec![],
        noop_hooks(),
        Arc::new(|_name, _args, cx| {
            cx.progress(Progress::Append("one".into()));
            cx.progress(Progress::Data(Tagged::new(&WorkspaceUse {
                name: "prog".into(),
            })));
            ToolOutput::ok("done")
        }),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "slowrun", json!({}))]),
            LlmResponse::text("ok"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    let (done, seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");

    let progress: Vec<&Value> = seen
        .iter()
        .filter(|v| v["method"] == "tool.progress")
        .collect();
    assert_eq!(progress.len(), 2, "{seen:?}");
    assert_eq!(progress[0]["params"]["call_id"], "c1");
    assert_eq!(progress[0]["params"]["branch"], branch.0.as_str());
    assert_eq!(progress[0]["params"]["append"], "one");
    assert_eq!(progress[1]["params"]["data"]["tag"], "workspace.use");
    assert_eq!(progress[1]["params"]["data"]["v"], 1);
    assert_eq!(progress[1]["params"]["data"]["data"]["name"], "prog");
}

// ---------- injection ----------

#[tokio::test]
async fn inject_ordering_without_dedup() {
    let plugin = Arc::new(plugin_with(
        "memory",
        &["plus"],
        vec![hook("always-inject", json!({ "event": kind::LLM_PRE }))],
        Arc::new(|_, _| inject(Some("card:x@a1b2c3d4"), "remember X")),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "plus", json!({}))]),
            LlmResponse::text("first turn done"),
            LlmResponse::text("second turn"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    turn(&kernel, &branch, &mut rx, "one").await;
    turn(&kernel, &branch, &mut rx, "two").await;

    // The kernel never deduplicates: one note per LLM call (2 + 1).
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let notes = nodes
        .iter()
        .filter(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "remember X"),
        )
        .count();
    assert_eq!(notes, 3);
    assert_eq!(llm.projections().len(), 3);

    // A prompt-hook import queued before the loop is flushed before the
    // llm.pre dispatch, so an llm.pre hook's own inject lands *after* it in
    // the same request (the update-notice ordering).
    let prompt_plugin = Arc::new(plugin_with(
        "memory",
        &[],
        vec![hook("import", json!({ "event": kind::PROMPT }))],
        Arc::new(|_, _| inject(Some("card:x@a1b2c3d4"), "# x\nimported body")),
        noop_tools(),
    ));
    let notify_plugin = Arc::new(plugin_with(
        "notifier",
        &[],
        vec![hook_prio(
            "card-updates",
            json!({ "event": kind::LLM_PRE }),
            i32::MAX,
        )],
        Arc::new(|_, _| inject(Some("card-update:x@e5f6a7b8"), "card x changed")),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![prompt_plugin, notify_plugin],
        vec![LlmResponse::text("done")],
    )
    .await;
    run_turn(&kernel, &branch, "hello").await;

    let projections = llm.projections();
    assert_eq!(projections.len(), 1);
    let texts: Vec<&str> = projections[0]
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::Note { content, .. } if !content.text.is_empty() => {
                Some(content.text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        ["# x\nimported body", "card x changed"],
        "import precedes update note in the same request"
    );
}

/// Plugins query the index instead of relying on kernel dedup: an effect
/// queued earlier in the batch is already visible, and repeating the same
/// payload counts twice (that both are written is
/// `inject_ordering_without_dedup`).
#[tokio::test]
async fn index_sees_queued_effects() {
    let plugin = Arc::new(plugin_with(
        "memory",
        &["append_marker", "probe_index"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new(
                    "c1",
                    "append_marker",
                    json!({"kind": "thing", "data": {"n": 1}}),
                ),
                ToolCall::new("c2", "probe_index", json!({"tag": "thing"})),
                ToolCall::new(
                    "c3",
                    "append_marker",
                    json!({"kind": "thing", "data": {"n": 1}}),
                ),
                ToolCall::new("c4", "probe_index", json!({"tag": "thing"})),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    assert_eq!(result_text(&nodes, "c2"), ("1".to_owned(), false));
    assert_eq!(result_text(&nodes, "c4"), ("2".to_owned(), false));
}

/// A `workspace.use` recorded earlier in the same tool batch is queued to the
/// safe point, but later calls of that batch must already see it — both the
/// plugin-facing state and the hook fields (a guard on the new default
/// workspace must not be bypassed by switching and acting in one batch).
#[tokio::test]
async fn queued_markers_are_visible_within_the_batch() {
    let plugin = Arc::new(plugin_with(
        "ws",
        &["append_marker", "probe_state", "write"],
        vec![hook(
            "card-guard",
            json!({ "event": kind::TOOL_PRE, "tool": "write", "paths": { "glob": "/cards/**" } }),
        )],
        block_hook("cards are guarded"),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new(
                    "c1",
                    "append_marker",
                    json!({"kind": "workspace.use", "data": {"name": "@rumu"}}),
                ),
                ToolCall::new("c2", "probe_state", json!({})),
                ToolCall::new(
                    "c3",
                    "write",
                    json!({"path": "@rumu:/cards/dev/x", "content": "x"}),
                ),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, state) = run_turn(&kernel, &branch, "switch and act").await;
    assert_eq!(result_text(&nodes, "c2"), ("@rumu".to_owned(), false));
    assert_eq!(
        result_text(&nodes, "c3"),
        ("Blocked by hook: cards are guarded".to_owned(), true)
    );
    // The marker itself still lands at the safe point, once.
    assert_eq!(state.default_workspace.as_deref(), Some("@rumu"));
    let uses = nodes
        .iter()
        .filter(|n| matches!(&n.body, NodeBody::Note { content, .. } if content.decode::<WorkspaceUse>().is_some()))
        .count();
    assert_eq!(uses, 1);
}

#[tokio::test]
async fn safe_point_queueing() {
    // Notes injected during a tool batch land after ALL ToolResults of that batch.
    let counter = Arc::new(Mutex::new(0u32));
    let c2 = counter.clone();
    let plugin = Arc::new(plugin_with(
        "memory",
        &["one", "two"],
        vec![hook("inject-per-call", json!({ "event": kind::TOOL_PRE }))],
        Arc::new(move |_, _| {
            let mut n = c2.lock().unwrap();
            *n += 1;
            inject(None, &format!("note-{}", *n))
        }),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new("c1", "one", json!({})),
                ToolCall::new("c2", "two", json!({})),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "run both").await;
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "assistant+calls",
            "tool_result",
            "tool_result", // both results before any note
            "note",
            "note",
            "assistant",
        ]
    );
}

// ---------- agent ----------

/// The child investigates and ends with a plain reply, whose text (plus a
/// transcript address) becomes the parent's tool result. Run for both call
/// orders: agent alone first, and agent after another call of the batch
/// (agent must not assume the head is its own assistant node).
#[tokio::test]
async fn agent_forks_a_child_and_survives_batch_order() {
    for calls in [
        vec![
            ToolCall::new(
                "e1",
                "agent",
                json!({"mode": "fork", "background": false, "task": "find the config file"}),
            ),
            ToolCall::new("o1", "other", json!({})),
        ],
        vec![
            ToolCall::new("o1", "other", json!({})),
            ToolCall::new(
                "e1",
                "agent",
                json!({"mode": "fork", "background": false, "task": "find the config file"}),
            ),
        ],
    ] {
        let plugin = Arc::new(plugin_with(
            "branch",
            &["agent", "other"],
            vec![],
            noop_hooks(),
            Arc::new(|_, _, _| ToolOutput::ok("other-done")),
        ));
        let (kernel, _llm, session, branch) = setup(
            vec![plugin],
            vec![
                LlmResponse::calls(calls),
                // child step 1: a reply without tool calls ends the sub-agent
                LlmResponse::text("config lives in /etc/rumu"),
                // parent step 2: final
                LlmResponse::text("all done"),
            ],
        )
        .await;

        let (_, nodes, _) = run_turn_on(&kernel, &branch, "look around").await;

        // Parent: agent result = child summary + transcript address; other
        // executed normally.
        let (e1_text, e1_err) = result_text(&nodes, "e1");
        assert!(
            e1_text.starts_with("config lives in /etc/rumu"),
            "{e1_text}"
        );
        assert!(
            e1_text.contains("\n\n[sub-agent: @rumu:/sessions/"),
            "branch address: {e1_text}"
        );
        assert!(!e1_err);
        assert_eq!(result_text(&nodes, "o1"), ("other-done".to_owned(), false));

        // Child branch: Done, kind Agent, answers for ALL calls of the head
        // assistant, then its own investigation.
        let branches = kernel.session_tree(&session).unwrap().2;
        let child = branches
            .iter()
            .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { task, .. } if task == "find the config file"))
            .expect("agent branch exists");
        assert_eq!(child.status, BranchStatus::Done);
        assert_eq!(child.forked_from, Some(nodes.get(2).unwrap().id.clone())); // assistant node

        let (cnodes, _) = kernel.branch_path(&child.id).unwrap();
        let kinds = body_kinds(&cnodes);
        assert_eq!(kinds[0], "marker[session.root]");
        assert_eq!(kinds[1], "user");
        assert_eq!(kinds[2], "assistant+calls");
        assert_eq!(kinds[3], "tool_result");
        assert_eq!(kinds[4], "tool_result");
        assert_eq!(kinds[5], "user");
        assert_eq!(kinds[6], "assistant");
        // The parent's batch is answered by placeholders; the goal is a
        // prompt of its own, not the `agent` call's result.
        let (child_e1, _) = result_text(&cnodes, "e1");
        assert!(!child_e1.contains("You are now a sub-agent"), "{child_e1}");
        let (child_o1, _) = result_text(&cnodes, "o1");
        assert!(child_o1.contains("left to the parent"), "{child_o1}");
        let NodeBody::User { content } = &cnodes[5].body else {
            unreachable!()
        };
        assert!(
            content
                .text
                .contains("You are now a sub-agent. Goal: find the config file")
        );
        assert!(
            content
                .text
                .contains("a reply without tool calls ends your turn")
        );
        assert!(content.text.contains("not yours to carry on"));

        // The address names the child's branch in this session.
        assert!(
            e1_text.contains(&rumu_core::branch_address(&session, &child.id)),
            "{e1_text}"
        );
    }
}

// ---------- agent limits ----------

/// `nodes_since` is the path from a user message on, plus the sub-agent
/// branches started from it — not those of earlier turns.
#[tokio::test]
async fn nodes_since_takes_in_the_sub_agents_started_after() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent", "other"],
        vec![],
        noop_hooks(),
        Arc::new(|_, _, _| ToolOutput::ok("done")),
    ));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "a1",
                "agent",
                json!({"mode": "fork", "background": false, "task": "look"}),
            )]),
            // the child's own call, then its reply
            LlmResponse::calls(vec![ToolCall::new("c1", "other", json!({}))]),
            LlmResponse::text("child done"),
            LlmResponse::text("first done"),
            LlmResponse::calls(vec![ToolCall::new("o2", "other", json!({}))]),
            LlmResponse::text("second done"),
        ],
    )
    .await;
    run_turn_on(&kernel, &branch, "first").await;
    let (_, nodes, _) = run_turn_on(&kernel, &branch, "second").await;
    let user = |text: &str| {
        nodes
            .iter()
            .find(|n| matches!(&n.body, NodeBody::User { content } if content.text == text))
            .unwrap()
            .id
            .clone()
    };
    let results = |from: &NodeId| -> Vec<String> {
        kernel
            .nodes_since(&branch, from)
            .unwrap()
            .iter()
            .filter_map(|n| match &n.body {
                NodeBody::ToolResult { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect()
    };
    let first = results(&user("first"));
    assert!(first.contains(&"c1".to_owned()), "{first:?}");
    assert!(first.contains(&"o2".to_owned()), "{first:?}");
    assert_eq!(results(&user("second")), ["o2"]);
}

// ---------- agent limits ----------

/// `context: "none"` forks at the session root: the child is seeded with the
/// task only, sees none of the parent conversation, and its transcript starts
/// at the task node.
#[tokio::test]
async fn agent_context_none_starts_at_the_root() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "background": false, "task": "independent work", "mode": "fresh" }),
            )]),
            LlmResponse::text("clean summary"),
            LlmResponse::text("parent done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn_on(&kernel, &branch, "look around").await;
    let root = nodes[0].id.clone();
    let child = kernel
        .session_tree(&session)
        .unwrap()
        .2
        .into_iter()
        .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { task, .. } if task == "independent work"))
        .expect("agent child");
    assert_eq!(child.forked_from, Some(root));

    let (cnodes, _) = kernel.branch_path(&child.id).unwrap();
    let kinds = body_kinds(&cnodes);
    assert_eq!(kinds[..2], ["marker[session.root]", "user"]);
    let NodeBody::User { content } = &cnodes[1].body else {
        panic!("the child's first own node is its task");
    };
    assert!(
        content.text.contains("You are now a sub-agent"),
        "{}",
        content.text
    );
    assert!(
        content.text.contains("independent work"),
        "{}",
        content.text
    );

    // The child's LLM projection held only the root and the task.
    let projections = llm.projections();
    assert_eq!(
        body_kinds(&projections[1]),
        vec!["marker[session.root]", "user"]
    );
    assert!(
        !projections[1].iter().any(|n| matches!(
            &n.body,
            NodeBody::User { content } if content.text == "look around"
        )),
        "the child must not see the parent's conversation"
    );

    // The result names the child's branch.
    let (text, err) = result_text(&nodes, "e1");
    assert!(!err, "{text}");
    let session = kernel.session_of(&child.id).unwrap();
    assert!(
        text.contains(&rumu_core::branch_address(&session, &child.id)),
        "{text}"
    );
}

/// An unknown `inherit.workspaces` name is a plain tool error: no child
/// branch is created.
#[tokio::test]
async fn agent_unknown_workspace_errors_without_a_branch() {
    let cat = project("proj", "/tmp/proj");
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup_with_workspaces(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "background": false, "task": "x", "workspaces": ["ghost"] }),
            )]),
            LlmResponse::text("done"),
        ],
        cat,
    )
    .await;

    let (_, nodes, _) = run_turn_on(&kernel, &branch, "go").await;
    let (text, err) = result_text(&nodes, "e1");
    assert!(err, "{text}");
    assert!(
        text.contains("unknown workspace `ghost` (available: proj, @scratch)"),
        "{text}"
    );
    assert_eq!(
        kernel.session_tree(&session).unwrap().2.len(),
        1,
        "no child branch"
    );
}

/// `inherit.workspaces` narrows what the child may address in both context
/// modes: the child gets a `workspace.set` marker with the named subset of
/// the parent's workspaces (a `proj`-only list here).
#[tokio::test]
async fn agent_workspaces_narrow_the_child_state() {
    let cat = Workspaces::new(vec![
        Workspace {
            name: "proj".into(),
            ssh: None,
            root: "/tmp/proj".into(),
            read_only: false,
        },
        Workspace {
            name: "other".into(),
            ssh: None,
            root: "/tmp/other".into(),
            read_only: false,
        },
    ]);
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup_with_workspaces(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({
                    "background": false, "task": "full",
                    "mode": "fork",
                    "workspaces": ["proj"]
                }),
            )]),
            LlmResponse::text("full child done"),
            LlmResponse::text("parent one done"),
            LlmResponse::calls(vec![ToolCall::new(
                "e2",
                "agent",
                json!({
                    "background": false, "task": "clean",
                    "mode": "fresh",
                    "workspaces": ["proj"]
                }),
            )]),
            LlmResponse::text("clean child done"),
            LlmResponse::text("parent two done"),
        ],
        cat,
    )
    .await;
    run_turn_on(&kernel, &branch, "go").await;
    run_turn_on(&kernel, &branch, "again").await;

    let tree = kernel.session_tree(&session).unwrap();
    for (task, set_at) in [("full", 3), ("clean", 1)] {
        let child = tree
            .2
            .iter()
            .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { task: t, .. } if t == task))
            .unwrap_or_else(|| panic!("agent child `{task}`"));
        let (cnodes, cstate) = kernel.branch_path(&child.id).unwrap();
        let kinds = body_kinds(&cnodes);
        assert_eq!(kinds[set_at], "marker[workspace.set]", "{task}: {kinds:?}");
        assert_eq!(
            cstate
                .workspaces
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            ["proj", "@scratch"],
            "{task}"
        );
        assert_eq!(cstate.restrict.as_deref(), Some(&["proj".to_owned()][..]));
        // Outside any project the (now narrower) default is `@scratch`.
        assert_eq!(
            cstate.default_workspace.as_deref(),
            Some(SCRATCH_WORKSPACE),
            "{task}"
        );
    }
}

/// A `context: "none"` child forked at the root still lands in the parent's
/// project: the project mount marker is seeded after the root, so the child
/// sees the project's workspaces and default.
#[tokio::test]
async fn agent_context_none_inherits_the_project() {
    let registry = registry_with(
        &[("app", "/s/app"), ("proj", "/p/proj")],
        "proj-deck",
        &["proj", "app"],
    );
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup_with_workspaces(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "background": false, "task": "clean", "mode": "fresh" }),
            )]),
            LlmResponse::text("child done"),
            LlmResponse::text("parent done"),
        ],
        registry,
    )
    .await;
    // Move into the project after the root, so the project is inherited as a
    // marker rather than already being a root default.
    kernel
        .move_project(&session, move_notes(None, Some("proj-deck"), "entered"))
        .await
        .unwrap();
    run_turn_on(&kernel, &branch, "go").await;

    let child = kernel
        .session_tree(&session)
        .unwrap()
        .2
        .into_iter()
        .find(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        .expect("agent child");
    let (cnodes, cstate) = kernel.branch_path(&child.id).unwrap();
    assert_eq!(
        body_kinds(&cnodes)[..3],
        ["marker[session.root]", "marker[card.mount]", "user"]
    );
    assert_eq!(cstate.project(), Some("proj-deck"));
    assert_eq!(cstate.mounted, ["proj", "app"]);
    assert_eq!(cstate.default_workspace.as_deref(), Some("proj"));
}

/// Agents nest at most three levels below main; the fourth call is refused
/// (and does not create a branch).
#[tokio::test]
async fn agent_depth_limit_refuses_the_fourth_level() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    // Depth-first LLM order: each level calls the next, then replies on the
    // way back up.
    let script = vec![
        LlmResponse::calls(vec![ToolCall::new(
            "g1",
            "agent",
            json!({ "mode": "fork", "background": false, "task": "two" }),
        )]),
        LlmResponse::calls(vec![ToolCall::new(
            "g2",
            "agent",
            json!({ "mode": "fork", "background": false, "task": "three" }),
        )]),
        LlmResponse::calls(vec![ToolCall::new(
            "g3",
            "agent",
            json!({ "mode": "fork", "background": false, "task": "four" }),
        )]),
        LlmResponse::calls(vec![ToolCall::new(
            "g4",
            "agent",
            json!({ "mode": "fork", "background": false, "task": "five" }),
        )]),
        LlmResponse::text("level 3 done"),
        LlmResponse::text("level 2 done"),
        LlmResponse::text("level 1 done"),
        LlmResponse::text("main done"),
    ];
    let (kernel, _llm, session, branch) = setup(vec![plugin], script).await;

    run_turn_on(&kernel, &branch, "go").await;
    let branches = kernel.session_tree(&session).unwrap().2;
    let agents: Vec<_> = branches
        .iter()
        .filter(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        .collect();
    assert_eq!(agents.len(), 3, "three levels; the fourth is refused");

    let deepest = agents
        .iter()
        .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { task, .. } if task == "four"))
        .expect("third-level agent");
    let (nodes, _) = kernel.branch_path(&deepest.id).unwrap();
    let (text, err) = result_text(&nodes, "g4");
    assert!(err, "{text}");
    assert!(
        text.contains("agent depth limit reached; do the work yourself"),
        "{text}"
    );
}

/// The concurrency gate serialises sub-agent loops (set to 1 here) even when
/// a script fans them out with `Promise.all`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_concurrency_limit_serialises_script_fanout() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["fanout"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let llm = Arc::new(TrackedLlm::new(
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "f1",
                "fanout",
                json!({ "tasks": ["a", "b"] }),
            )]),
            LlmResponse::text("child a"),
            LlmResponse::text("child b"),
            LlmResponse::text("parent done"),
        ],
        Duration::from_millis(80),
    ));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![plugin],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    kernel.set_agent_concurrency(1);

    run_turn_on(&kernel, &branch, "go").await;
    assert_eq!(
        llm.max_active.load(Ordering::SeqCst),
        1,
        "with one permit the two sub-agents cannot overlap"
    );
    let agents = kernel
        .session_tree(&session.id)
        .unwrap()
        .2
        .into_iter()
        .filter(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        .count();
    assert_eq!(agents, 2);
}

/// Aborting while an agent waits for a permit must not start its child.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abort_while_waiting_for_an_agent_permit_creates_no_child() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["fanout"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let llm = Arc::new(TrackedLlm::new(
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "f1",
                "fanout",
                json!({ "tasks": ["a", "b"] }),
            )]),
            LlmResponse::text("child a"),
            LlmResponse::text("child b"),
        ],
        Duration::from_secs(120),
    ));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm,
        vec![plugin],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    kernel.set_agent_concurrency(1);
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();

    // Wait until exactly one child holds the permit (the other is waiting).
    wait_until(Duration::from_secs(5), || {
        agent_branches(&kernel, &session.id).len() == 1
    })
    .await;
    kernel.abort(&branch).unwrap();
    let done = wait_turn(&mut rx, Some(&branch.0)).await;
    assert_eq!(done["method"], "turn.done", "{done}");

    // The running child is cancelled by its parent token and closes out; the
    // waiter sees the cancellation before the freed permit and starts nothing.
    wait_until(Duration::from_secs(5), || {
        let agents = agent_branches(&kernel, &session.id);
        agents.len() == 1 && agents.iter().all(|b| b.status == BranchStatus::Done)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        agent_branches(&kernel, &session.id).len(),
        1,
        "the waiting agent never created a branch"
    );
}

fn agent_branches(kernel: &Arc<Kernel>, session: &SessionId) -> Vec<rumu_core::Branch> {
    kernel
        .session_tree(session)
        .unwrap()
        .2
        .into_iter()
        .filter(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        .collect()
}

/// Poll `check` until true or `timeout` expires.
async fn wait_until(timeout: Duration, check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------- abort ----------

/// Abort marks every call still pending — direct ones and ones wrapped in a
/// `code` script — as aborted, and the tree stays valid.
#[tokio::test]
async fn abort_appends_error_results() {
    for inner in [false, true] {
        let plugin: Arc<dyn Plugin> = if inner {
            Arc::new(script_host(&["sleep"], vec![], noop_hooks(), noop_tools()))
        } else {
            Arc::new(plugin_with(
                "slow",
                &["sleep"],
                vec![],
                noop_hooks(),
                noop_tools(),
            ))
        };
        let calls = if inner {
            LlmResponse::calls(vec![
                ToolCall::new(
                    "c1",
                    "script",
                    json!({"calls": [{"name": "sleep", "args": {}}]}),
                ),
                ToolCall::new("c2", "sleep", json!({})),
            ])
        } else {
            LlmResponse::calls(vec![
                ToolCall::new("c1", "sleep", json!({})),
                ToolCall::new("c2", "sleep", json!({})),
            ])
        };
        let (kernel, _llm, _s, branch) = setup(vec![plugin], vec![calls]).await;
        let mut rx = kernel.tap_pushes();

        kernel.prompt(&branch, "sleep forever").await.unwrap();
        // Wait until the assistant node with tool calls is appended.
        while let Some(v) = rx.recv().await {
            if v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant"
            {
                break;
            }
        }
        kernel.abort(&branch).unwrap();
        let done = wait_turn(&mut rx, None).await;
        assert_eq!(done["method"], "turn.done");

        let (nodes, _) = kernel.branch_path(&branch).unwrap();
        assert_eq!(
            result_text(&nodes, "c1"),
            ("aborted".to_owned(), true),
            "inner={inner}"
        );
        assert_eq!(result_text(&nodes, "c2"), ("aborted".to_owned(), true));
    }
}

// ---------- workspaces ----------

fn workspace_names(st: &PathState) -> Vec<&str> {
    st.workspaces.iter().map(|w| w.name.as_str()).collect()
}

/// Every registered workspace is addressable from any session. A project's
/// mounted list is the subset its deck names, in the deck's order (names
/// that left the registry drop out), and its first is the default.
#[tokio::test]
async fn every_workspace_is_addressable_and_the_project_mounts_its_own() {
    let registry = registry_with(
        &[("app", "/s/app"), ("lib", "/l/lib"), ("proj", "/p/proj")],
        "proj-deck",
        &["proj", "ghost", "lib"],
    );
    let (kernel, _llm) = kernel_with(vec![], vec![], registry);

    // Outside a project: the whole registry is addressable, none is mounted,
    // `@scratch` is the default.
    let (s, _) = kernel.create_session("assistant".into()).await.unwrap();
    let (_, st) = kernel.branch_path(&s.main_branch).unwrap();
    assert_eq!(workspace_names(&st), ["app", "lib", "proj", "@scratch"]);
    assert!(st.mounted.is_empty() && st.project().is_none());
    assert_eq!(st.default_workspace.as_deref(), Some(SCRATCH_WORKSPACE));
    assert_eq!(s.project, None);

    // In a project: its registered mounts, in its order, first is default.
    // `ghost` is mounted but unregistered, so it is not addressable.
    let (kernel, _llm, _s, branch) = setup_in_project(
        vec![],
        vec![],
        registry_with(
            &[("app", "/s/app"), ("lib", "/l/lib"), ("proj", "/p/proj")],
            "proj-deck",
            &["proj", "ghost", "lib"],
        ),
        "proj-deck",
    )
    .await;
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(workspace_names(&st), ["app", "lib", "proj", "@scratch"]);
    assert_eq!(st.mounted, ["proj", "lib"]);
    assert_eq!(st.project(), Some("proj-deck"));
    assert_eq!(st.default_workspace.as_deref(), Some("proj"));
}

/// A registry edit reaches running sessions and is pushed to clients as
/// `workspaces.updated`, carrying both lists.
#[tokio::test]
async fn registry_changes_are_pushed_with_projects() {
    let registry = registry_with(&[("app", "/s/app")], "proj-deck", &["app"]);
    let (kernel, _llm, _s, branch) =
        setup_in_project(vec![], vec![], registry.clone(), "proj-deck").await;
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(workspace_names(&st), ["app", "@scratch"]);
    assert_eq!(st.mounted, ["app"]);

    let mut rx = kernel.tap_pushes();
    registry
        .update_both(|list, projects| {
            list.push(Workspace {
                name: "lib".into(),
                ssh: None,
                root: "/l/lib".into(),
                read_only: false,
            });
            projects.insert("proj-deck".into(), vec!["lib".into(), "app".into()]);
            Ok::<_, ()>(())
        })
        .unwrap();

    let push = loop {
        let v = rx.recv().await.expect("a workspaces.updated push");
        if v["method"] == "workspaces.updated" {
            break v;
        }
    };
    assert_eq!(push["params"]["workspaces"][0]["name"], "app");
    assert_eq!(push["params"]["workspaces"][1]["name"], "lib");
    assert_eq!(
        push["params"]["projects"]["proj-deck"],
        json!(["lib", "app"])
    );

    // The running branch sees the new registry and the reordered mounts.
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(workspace_names(&st), ["app", "lib", "@scratch"]);
    assert_eq!(st.mounted, ["lib", "app"]);
    assert_eq!(st.default_workspace.as_deref(), Some("lib"));
}

/// `move_project` writes its notes in order, and the last one says where
/// the default workspace moved when it did.
#[tokio::test]
async fn move_project_records_notes_and_the_default_moved() {
    let registry = registry_with(
        &[("app", "/s/app"), ("proj", "/p/proj")],
        "proj-deck",
        &["proj", "app"],
    );
    registry
        .update_both(|_, projects| {
            projects.insert("other".into(), vec!["app".into()]);
            projects.insert("empty".into(), Vec::new());
            Ok::<_, ()>(())
        })
        .unwrap();
    let (kernel, _llm) = kernel_with(vec![], vec![], registry);
    let (s, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = s.main_branch.clone();
    let mut rx = kernel.tap_pushes();
    let last_text = |kernel: &Arc<Kernel>| {
        let nodes = kernel.branch_path(&branch).unwrap().0;
        match &nodes.last().unwrap().body {
            NodeBody::Note { content, .. } => content.text.clone(),
            _ => panic!("a note"),
        }
    };

    kernel
        .move_project(&s.id, move_notes(None, Some("proj-deck"), "in."))
        .await
        .unwrap();
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(st.project(), Some("proj-deck"));
    assert_eq!(st.mounted, ["proj", "app"]);
    assert_eq!(st.default_workspace.as_deref(), Some("proj"));
    assert_eq!(
        last_text(&kernel),
        "in. The default workspace is now `proj`."
    );
    let info = kernel.session_tree(&s.id).unwrap().0;
    assert_eq!(info.project.as_deref(), Some("proj-deck"));
    assert!(std::iter::from_fn(|| rx.try_recv().ok()).any(|v| {
        v["method"] == "session.updated" && v["params"]["session"]["project"] == "proj-deck"
    }));

    // Switching: the old project goes, the new one comes.
    kernel
        .move_project(
            &s.id,
            move_notes(Some("proj-deck"), Some("other"), "moved."),
        )
        .await
        .unwrap();
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert_eq!(st.project(), Some("other"));
    assert_eq!(st.mounts.iter().filter(|m| m.project).count(), 1);
    assert_eq!(
        last_text(&kernel),
        "moved. The default workspace is now `app`."
    );
    // To one without workspaces: `@scratch`.
    kernel
        .move_project(&s.id, move_notes(Some("other"), Some("empty"), "moved."))
        .await
        .unwrap();
    assert_eq!(
        last_text(&kernel),
        "moved. The default workspace is now `@scratch`."
    );
    // Out of a project with the same default: nothing more said.
    kernel
        .move_project(&s.id, move_notes(Some("empty"), None, ""))
        .await
        .unwrap();
    let (_, st) = kernel.branch_path(&branch).unwrap();
    assert!(st.project().is_none() && st.mounted.is_empty());
    assert_eq!(st.default_workspace.as_deref(), Some(SCRATCH_WORKSPACE));
    assert_eq!(last_text(&kernel), "left");
    assert_eq!(kernel.session_tree(&s.id).unwrap().0.project, None);
}

/// `SessionInfo.project` tracks the main line: created, moved into a
/// project, moved back before that and forward again, and copied by
/// `fork_session`.
#[tokio::test]
async fn session_project_follows_the_main_line() {
    let registry = registry_with(
        &[("app", "/s/app"), ("proj", "/p/proj")],
        "proj-deck",
        &["proj"],
    );
    let (kernel, _llm) = kernel_with(vec![], vec![], registry);
    let (s, _) = kernel.create_session("assistant".into()).await.unwrap();
    let main = s.main_branch.clone();
    let root = kernel.branch_path(&main).unwrap().0[0].id.clone();

    // Moving the main line into a project is recorded on the session.
    kernel
        .move_project(&s.id, move_notes(None, Some("proj-deck"), "in"))
        .await
        .unwrap();
    assert_eq!(
        kernel.session_tree(&s.id).unwrap().0.project.as_deref(),
        Some("proj-deck")
    );

    let head = kernel
        .branch_path(&main)
        .unwrap()
        .0
        .last()
        .unwrap()
        .id
        .clone();

    // Moving the head back before it leaves the project; forward again
    // returns to it.
    kernel.move_head(&main, &root, false).await.unwrap();
    assert_eq!(kernel.session_tree(&s.id).unwrap().0.project, None);
    kernel.move_head(&main, &head, false).await.unwrap();
    assert_eq!(
        kernel.session_tree(&s.id).unwrap().0.project.as_deref(),
        Some("proj-deck")
    );

    // `fork_session` copies the project from the path it copies.
    let copy = kernel.fork_session(&head).unwrap();
    assert_eq!(copy.project.as_deref(), Some("proj-deck"));
    let copy_none = kernel.fork_session(&root).unwrap();
    assert_eq!(copy_none.project, None);
}

#[tokio::test]
async fn delete_session_removes_the_tree_and_pushes_it() {
    let (kernel, _llm, session, branch) = setup(vec![], vec![]).await;
    // Another session must be untouched.
    let (keep, _) = kernel.create_session("assistant".into()).await.unwrap();
    let mut rx = kernel.tap_pushes();

    kernel.delete_session(&session).unwrap();

    let push = rx.recv().await.unwrap();
    assert_eq!(push["method"], "session.deleted");
    assert_eq!(push["params"]["session"]["id"], session.0);

    let list = kernel.list_sessions().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, keep.id);
    assert!(kernel.session_tree(&session).is_err());
    assert!(kernel.branch_path(&branch).is_err());

    // An unknown id is a clear error, not a silent success.
    assert!(matches!(
        kernel.delete_session(&SessionId::new()),
        Err(rumu_kernel::Error::SessionNotFound(_))
    ));
}

/// Deleting a session removes its `@scratch` directory; a session whose
/// scratch was never created still deletes cleanly.
#[tokio::test]
async fn delete_session_removes_the_scratch_directory() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Workspaces::default().with_scratch(dir.path().to_path_buf());
    let (kernel, _llm) = kernel_with(vec![], vec![], registry.clone());
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();

    let scratch = registry.scratch(&session.id).root;
    std::fs::create_dir_all(scratch.as_str()).unwrap();
    std::fs::write(
        std::path::Path::new(scratch.as_str()).join("note.txt"),
        "hi",
    )
    .unwrap();

    kernel.delete_session(&session.id).unwrap();
    assert!(!std::path::Path::new(scratch.as_str()).exists());

    // Never created: deleting succeeds (the removal ignores `NotFound`).
    let (other, _) = kernel.create_session("assistant".into()).await.unwrap();
    let other_scratch = registry.scratch(&other.id).root;
    assert!(!std::path::Path::new(other_scratch.as_str()).exists());
    kernel.delete_session(&other.id).unwrap();
}

/// Fail-fast: a malformed matcher fails kernel construction instead of
/// silently never matching. (The three bad shapes themselves are unit-tested
/// in `predicate::compile_rejects_bad_matchers`; here we only check that the
/// error is attributed to the owning hook.)
#[tokio::test]
async fn invalid_matchers_fail_kernel_construction() {
    let plugin = Arc::new(plugin_with(
        "bad",
        &[],
        vec![hook("broken", json!({ "toolz": "bash" }))],
        noop_hooks(),
        noop_tools(),
    ));
    let err = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(FakeLlm::new(vec![])),
        vec![plugin],
        Workspaces::default(),
    )
    .err()
    .expect("kernel construction must fail on an invalid matcher");
    let msg = format!("{err:#}");
    assert!(msg.contains("hook bad/broken"), "{msg}");
}

/// A guard on the `workspace` field catches path-less calls (`bash`) in the
/// target workspace.
#[tokio::test]
async fn workspace_field_guards_pathless_calls() {
    let plugin = Arc::new(guard(
        &["bash"],
        vec![hook(
            "no-bash-in-proj",
            json!({ "event": kind::TOOL_PRE, "workspace": "proj" }),
        )],
        "proj is protected",
    ));
    let (kernel, _llm, _s, branch) = setup_in_project(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "bash", json!({"command": "ls"}))]),
            LlmResponse::text("ok"),
        ],
        registry_with(&[("proj", "/p/rumu")], "proj-deck", &["proj"]),
        "proj-deck",
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "list things").await;
    // `bash` carries no `path`; the resolved target (`proj`, the branch
    // default) is still matchable — and blockable.
    assert_eq!(
        result_text(&nodes, "c1"),
        ("Blocked by hook: proj is protected".to_owned(), true)
    );
}

// ---------- two-phase dispatch: rewrite vs. guard ----------

/// A shim that rewrites any `shell` command to `rewrite_to`, plus a guard
/// that blocks destructive commands. The guard must run again on the final
/// payload: a stale "allowed" verdict must not let the rewritten command run,
/// and a stale "blocked" verdict must not survive a sanitized rewrite.
fn shim(rewrite_to: &str) -> TestPlugin {
    let rewrite_to = rewrite_to.to_owned();
    plugin_with(
        "shim",
        &["shell"],
        vec![
            hook_prio(
                "guard",
                json!({ "event": kind::TOOL_PRE, "tool": "shell" }),
                5,
            ),
            hook_prio(
                "rewrite",
                json!({ "event": kind::TOOL_PRE, "tool": "shell" }),
                10,
            ),
        ],
        Arc::new(move |id, ev| {
            if id == "rewrite" {
                rewrite(json!({ "args": { "command": rewrite_to.clone() } }))
            } else if ev
                .fields
                .cmd
                .as_deref()
                .is_some_and(|c| c.starts_with("rm -rf"))
            {
                block("rm -rf is forbidden")
            } else {
                pass()
            }
        }),
        Arc::new(|_name, args, _| {
            ToolOutput::ok(
                args.get("command")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_owned(),
            )
        }),
    )
}

#[tokio::test]
async fn two_phase_dispatch() {
    for (rewrite_to, prompt_cmd, text, is_error, ran) in [
        (
            "rm -rf /",
            "echo hi",
            "Blocked by hook: rm -rf is forbidden",
            true,
            0,
        ),
        ("echo cleaned", "rm -rf /tmp/x", "echo cleaned", false, 1),
    ] {
        let plugin = Arc::new(shim(rewrite_to));
        let (kernel, _llm, _s, branch) = setup(
            vec![plugin.clone()],
            vec![
                LlmResponse::calls(vec![ToolCall::new(
                    "c1",
                    "shell",
                    json!({"command": prompt_cmd}),
                )]),
                LlmResponse::text("ok"),
            ],
        )
        .await;

        let (_, nodes, _) = run_turn(&kernel, &branch, "run it").await;
        assert_eq!(
            result_text(&nodes, "c1"),
            (text.to_owned(), is_error),
            "rewrite_to={rewrite_to}"
        );
        assert_eq!(plugin.calls().len(), ran, "rewrite_to={rewrite_to}");
        if !is_error {
            assert_eq!(plugin.calls()[0].1["command"], "echo cleaned");
        }
    }
}

/// Fail-closed: an intercept hook that times out on `tool.pre` becomes a
/// synthetic block instead of a silent no-op.
#[tokio::test]
async fn intercept_hook_timeout_fails_closed() {
    let mut plugin = guard(
        &["shell"],
        vec![hook(
            "slow",
            json!({ "event": kind::TOOL_PRE, "tool": "shell" }),
        )],
        "unused",
    );
    plugin.delay = Duration::from_millis(300);
    let plugin = Arc::new(plugin);
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "shell", json!({"command": "ls"}))]),
            LlmResponse::text("ok"),
        ],
    )
    .await;
    kernel.set_hook_timeout(Duration::from_millis(50));

    let (_, nodes, _) = run_turn(&kernel, &branch, "list files").await;
    let (text, err) = result_text(&nodes, "c1");
    assert!(err, "{text}");
    assert!(text.contains("timed out"), "{text}");
    assert!(plugin.calls().is_empty());
}

// ---------- code mode: internal calls ----------

#[tokio::test]
async fn code_inner_block_is_a_trace_node() {
    let plugin = Arc::new(script_host(
        &["secret"],
        vec![hook(
            "block-secret",
            json!({ "event": kind::TOOL_PRE, "tool": "secret", "caller": "script-host" }),
        )],
        block_hook("secret stays out"),
        noop_tools(),
    ));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": [{ "name": "secret", "args": {} }] }),
            )]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    let (_, _seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let invokes: Vec<&Node> = nodes
        .iter()
        .filter(|n| {
            matches!(
                &n.body,
                NodeBody::ToolResult {
                    invoke: Some(_),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(invokes.len(), 1, "the invoke is an ordinary node");
    let NodeBody::ToolResult {
        call_id,
        content,
        is_error,
        invoke: Some(invoke),
    } = &invokes[0].body
    else {
        unreachable!()
    };
    assert_eq!(call_id, "c1.1");
    assert_eq!(invoke.parent, "c1");
    assert_eq!(invoke.name, "secret");
    assert_eq!(invoke.blocked.as_deref(), Some("secret stays out"));
    assert!(*is_error);
    assert_eq!(content.text, "Blocked by hook: secret stays out");
    let (text, err) = result_text(&nodes, "c1");
    assert!(!err, "{text}");
    let rows: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["ok"], false);
    assert_eq!(rows[0]["blocked"], "secret stays out");
    assert_eq!(rows[0]["text"], "Blocked by hook: secret stays out");
    assert!(
        plugin.calls().iter().all(|(name, _)| name != "secret"),
        "the blocked tool never ran: {:?}",
        plugin.calls()
    );
}

/// The trace keeps a `write`/`edit` file patch so the UI can render the diff
/// after a reload, but no other structured result.
#[tokio::test]
async fn code_invoke_keeps_the_file_patch() {
    let write: ToolFn = Arc::new(|_: &str, _: Value, _: &Cx| {
        let mut out = ToolOutput::ok("wrote a.rs (1 bytes)");
        out.content.data = Some(Tagged {
            tag: rumu_core::FILE_PATCH_TAG.to_owned(),
            v: 1,
            data: json!({ "kind": "update", "hunks": null }),
        });
        out
    });
    let plugin = Arc::new(script_host(&["write"], vec![], noop_hooks(), write));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": [{
                    "name": "write",
                    "args": { "path": "a.rs", "content": "b" },
                }] }),
            )]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    let (_, _seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let node = nodes
        .iter()
        .find(|n| {
            matches!(
                &n.body,
                NodeBody::ToolResult {
                    invoke: Some(_),
                    ..
                }
            )
        })
        .expect("the invoke node");
    let NodeBody::ToolResult {
        content, invoke, ..
    } = &node.body
    else {
        unreachable!()
    };
    assert_eq!(invoke.as_ref().unwrap().name, "write");
    let patch = content.data.as_ref().expect("the file patch is kept");
    assert_eq!(patch.tag, rumu_core::FILE_PATCH_TAG);
    assert_eq!(patch.v, 1);
    assert_eq!(patch.data["kind"], "update");
    assert!(content.attachments.is_empty());
}

/// A file tool's or `bash`'s result names the workspace the call addressed,
/// written or defaulted, unless the tool left a payload of its own — on
/// direct and on `code` calls alike.
#[tokio::test]
async fn results_name_the_workspace_addressed() {
    let tools: ToolFn = Arc::new(|name: &str, _: Value, _: &Cx| {
        let mut out = ToolOutput::ok("ok");
        if name == "write" {
            out.content.data = Some(Tagged {
                tag: rumu_core::FILE_PATCH_TAG.to_owned(),
                v: 1,
                data: json!({ "kind": "create" }),
            });
        }
        out
    });
    let plugin = Arc::new(script_host(
        &["read", "bash", "write"],
        vec![],
        noop_hooks(),
        tools,
    ));
    let (kernel, _llm, _session, branch) = setup_with_workspaces(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new("c1", "read", json!({ "path": "proj:src/a.rs" })),
                ToolCall::new("c2", "bash", json!({ "command": "ls" })),
                ToolCall::new("c3", "write", json!({ "path": "b.rs", "content": "" })),
                ToolCall::new(
                    "c4",
                    "script",
                    json!({ "calls": [{ "name": "bash", "args": { "command": "ls" } }] }),
                ),
            ]),
            LlmResponse::text("done"),
        ],
        project("proj", "/p"),
    )
    .await;
    let mut rx = kernel.tap_pushes();
    let (_, _seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let data = |id: &str| {
        nodes
            .iter()
            .find_map(|n| match &n.body {
                NodeBody::ToolResult {
                    call_id, content, ..
                } if call_id == id => Some(content.data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no result for {id}"))
    };
    let target = |id: &str| data(id).and_then(|d| d.decode::<WorkspaceTarget>());
    assert_eq!(target("c1").unwrap().ws, "proj");
    assert_eq!(target("c2").unwrap().ws, SCRATCH_WORKSPACE);
    assert_eq!(data("c3").unwrap().tag, rumu_core::FILE_PATCH_TAG);
    assert_eq!(target("c4.1").unwrap().ws, SCRATCH_WORKSPACE);
    assert!(target("c4").is_none(), "`code` addresses no workspace");
}

/// `caller` distinguishes inner (`code`) from direct (`model`) calls in both
/// directions, and direct calls are the default the hooks see.
#[tokio::test]
async fn caller_field_separates_inner_and_direct_calls() {
    for block_caller in ["script-host", "model"] {
        let fields = Arc::new(Mutex::new(Vec::<(Option<String>, Option<String>)>::new()));
        let record = fields.clone();
        let plugin = Arc::new(TestPlugin::new(
            "script-host",
            vec![tool_spec("script"), tool_spec("plus")],
            vec![
                hook("record", json!({ "event": kind::TOOL_PRE })),
                hook(
                    "guard",
                    json!({ "event": kind::TOOL_PRE, "tool": "plus", "caller": block_caller }),
                ),
            ],
            Arc::new(move |id, ev| {
                if id == "record" {
                    record
                        .lock()
                        .unwrap()
                        .push((ev.fields.tool.clone(), ev.fields.caller.clone()));
                    pass()
                } else {
                    block("guarded")
                }
            }),
            plus_tool(),
        ));
        let (kernel, _llm, _s, branch) = setup(
            vec![plugin.clone()],
            vec![
                LlmResponse::calls(vec![
                    ToolCall::new(
                        "c1",
                        "script",
                        json!({ "calls": [{ "name": "plus", "args": { "a": 1, "b": 2 } }] }),
                    ),
                    ToolCall::new("c2", "plus", json!({ "a": 1, "b": 2 })),
                ]),
                LlmResponse::text("done"),
            ],
        )
        .await;

        let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;

        // Direct calls default to "model"; plugin invokes name the plugin.
        assert_eq!(
            fields.lock().unwrap().as_slice(),
            [
                (Some("script".to_owned()), Some("model".to_owned())),
                (Some("plus".to_owned()), Some("script-host".to_owned())),
                (Some("plus".to_owned()), Some("model".to_owned())),
            ],
            "block_caller={block_caller}"
        );

        let (text, _) = result_text(&nodes, "c1");
        let rows: Value = serde_json::from_str(&text).unwrap();
        let direct = result_text(&nodes, "c2");
        if block_caller == "script-host" {
            assert_eq!(rows[0]["blocked"], "guarded");
            assert_eq!(direct, ("3".to_owned(), false));
        } else {
            assert_eq!(rows[0]["text"], "3");
            assert_eq!(direct, ("Blocked by hook: guarded".to_owned(), true));
        }
        // Only the unblocked direct call reached the plugin.
        assert_eq!(
            plugin
                .calls()
                .iter()
                .filter(|(name, _)| name == "plus")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn rewrite_cannot_forge_caller() {
    let plugin = Arc::new(TestPlugin::new(
        "script-host",
        vec![tool_spec("script"), tool_spec("plus")],
        vec![
            hook_prio(
                "forge",
                json!({ "event": kind::TOOL_PRE, "tool": "plus" }),
                0,
            ),
            hook_prio(
                "guard",
                json!({ "event": kind::TOOL_PRE, "tool": "plus", "caller": "script-host" }),
                10,
            ),
        ],
        Arc::new(|id, _ev| {
            if id == "forge" {
                rewrite(json!({ "caller": "model" }))
            } else {
                block("inner only")
            }
        }),
        plus_tool(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new(
                    "c1",
                    "script",
                    json!({ "calls": [{ "name": "plus", "args": { "a": 2, "b": 3 } }] }),
                ),
                ToolCall::new("c2", "plus", json!({ "a": 2, "b": 3 })),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    // The guard still sees caller = script-host after the rewrite.
    let (text, _) = result_text(&nodes, "c1");
    let rows: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["blocked"], "inner only");
    // The direct call's forged caller was removed too: it is not "script-host".
    assert_eq!(result_text(&nodes, "c2"), ("5".to_owned(), false));
}

#[tokio::test]
async fn rewrite_cannot_smuggle_a_denied_tool_into_code() {
    let plugin = Arc::new(script_host(
        &["plus"],
        vec![hook(
            "smuggle",
            json!({ "event": kind::TOOL_PRE, "tool": "plus", "caller": "script-host" }),
        )],
        Arc::new(|_, _| rewrite(json!({ "name": "code" }))),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": [{ "name": "plus", "args": {} }] }),
            )]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    // The denied check runs again on the effective name: no nested `code`.
    let (text, _) = result_text(&nodes, "c1");
    let rows: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["ok"], false);
    assert!(
        rows[0]["text"]
            .as_str()
            .unwrap()
            .contains("not available from a plugin invoke"),
        "{}",
        rows[0]
    );
    assert_eq!(
        plugin
            .calls()
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["script"]
    );
}

#[tokio::test]
async fn code_inner_inject_is_queued() {
    let plugin = Arc::new(script_host(
        &["secret", "append_note"],
        vec![hook(
            "inject",
            json!({ "event": kind::TOOL_PRE, "tool": "secret", "caller": "script-host" }),
        )],
        Arc::new(|_, _| inject(Some("k"), "inner note")),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new(
                    "c1",
                    "script",
                    json!({ "calls": [{ "name": "secret", "args": {} }] }),
                ),
                ToolCall::new("c2", "append_note", json!({})),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let note_at = nodes
        .iter()
        .position(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "inner note"),
        )
        .expect("injected note on the path");
    let c1_at = nodes
        .iter()
        .position(|n| matches!(&n.body, NodeBody::ToolResult { call_id, .. } if call_id == "c1"))
        .unwrap();
    assert!(
        note_at > c1_at,
        "the note lands after the outer tool result"
    );
    // The inner note and the direct call's own note are both written.
    let notes = nodes
        .iter()
        .filter(|n| {
            matches!(&n.body, NodeBody::Note { content, .. }
            if content.text == "inner note" || content.text == "direct note")
        })
        .count();
    assert_eq!(notes, 2);
    assert_eq!(result_text(&nodes, "c2"), ("noted".to_owned(), false));
}

#[tokio::test]
async fn code_inner_workspace_use_is_visible_to_the_next_inner_call() {
    let plugin = Arc::new(script_host(
        &["set_ws", "probe_state"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": [
                    { "name": "set_ws", "args": { "name": "@rumu" } },
                    { "name": "probe_state", "args": {} }
                ] }),
            )]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, state) = run_turn(&kernel, &branch, "go").await;
    let (text, _) = result_text(&nodes, "c1");
    let rows: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["text"], "set");
    assert_eq!(
        rows[1]["text"], "@rumu",
        "the queued workspace.use marker is already in effect"
    );
    assert_eq!(state.default_workspace.as_deref(), Some("@rumu"));
    let markers = nodes
        .iter()
        .filter(|n| matches!(&n.body, NodeBody::Note { content, .. } if content.decode::<WorkspaceUse>().is_some()))
        .count();
    assert_eq!(markers, 1, "the marker still lands once, at the safe point");
}

#[tokio::test]
async fn code_denied_tools_skip_hooks() {
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let record = seen.clone();
    let plugin = Arc::new(script_host(
        &["plus"],
        vec![hook("record", json!({ "event": kind::TOOL_PRE }))],
        Arc::new(move |_, ev| {
            if let Some(tool) = &ev.fields.tool {
                record.lock().unwrap().push(tool.clone());
            }
            pass()
        }),
        noop_tools(),
    ));
    let denied = ["code"];
    let calls: Vec<Value> = denied
        .iter()
        .map(|name| json!({ "name": name, "args": {} }))
        .collect();
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": calls }),
            )]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    let (_, _pushes) = turn_collect(&kernel, &branch, &mut rx, "go").await;

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let (text, _) = result_text(&nodes, "c1");
    let rows: Value = serde_json::from_str(&text).unwrap();
    for (i, name) in denied.iter().enumerate() {
        assert_eq!(rows[i]["name"], *name);
        assert_eq!(rows[i]["ok"], false);
        assert!(
            rows[i]["text"]
                .as_str()
                .unwrap()
                .contains("not available from a plugin invoke"),
            "{name}: {}",
            rows[i]
        );
    }
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["script"],
        "no hook fires for denied tools"
    );
    assert!(
        nodes.iter().all(|n| !matches!(
            &n.body,
            NodeBody::ToolResult {
                invoke: Some(_),
                ..
            }
        )),
        "denied calls leave no trace node"
    );
}

#[tokio::test]
async fn host_call_tool_outside_a_batch_errors() {
    let (kernel, _llm, _s, branch) = setup(vec![], vec![]).await;
    let err = kernel
        .runtime()
        .invoke(&branch, "script-host", "c1", 1, "plus", json!({}))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("no running turn"), "{err:#}");
}

/// Regenerating moves the main line onto the prompt: the same branch replies
/// again from there without appending the prompt a second time, and the old
/// reply stays in the tree as a variant.
#[tokio::test]
async fn regenerate_moves_the_main_line() {
    let (kernel, llm, session, branch) = setup(
        vec![],
        vec![
            LlmResponse::text("first reply"),
            LlmResponse::text("second reply"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "hello").await;
    let prompt = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::User { .. }))
        .expect("the turn appended a user node")
        .id
        .clone();

    let mut rx = kernel.tap_pushes();
    let regenerated = kernel
        .regenerate(&prompt, Default::default(), false)
        .await
        .unwrap();
    wait_turn(&mut rx, Some(regenerated.id.0.as_str())).await;

    let (info, nodes, branches) = kernel.session_tree(&session).unwrap();
    assert_eq!(regenerated.id, branch);
    assert_eq!(info.main_branch, branch);
    assert_eq!(branches.len(), 1);
    let replies: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::Assistant { content, .. } if n.parent.as_ref() == Some(&prompt) => {
                Some(content.text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 2, "both replies hang off the prompt");
    let (path, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        body_kinds(&path),
        vec!["marker[session.root]", "user", "assistant"]
    );
    assert!(matches!(&path.last().unwrap().body,
        NodeBody::Assistant { content, .. } if content.text == "second reply"));
    // The model was asked with the prompt exactly once.
    let projections = llm.projections();
    let users = projections
        .last()
        .unwrap()
        .iter()
        .filter(|n| matches!(n.body, NodeBody::User { .. }))
        .count();
    assert_eq!(users, 1);
}

/// Moving the main line keeps every node: back to the prompt, then on to
/// the reply it left, and the head is where it was.
#[tokio::test]
async fn moving_the_main_line_keeps_what_it_leaves() {
    let (kernel, _llm, session, branch) = setup(vec![], vec![LlmResponse::text("reply")]).await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "hello").await;
    let prompt = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::User { .. }))
        .expect("the turn appended a user node")
        .id
        .clone();
    let reply = nodes.last().unwrap().id.clone();

    let moved = kernel.move_head(&branch, &prompt, false).await.unwrap();
    assert_eq!(
        (moved.id.clone(), moved.head.clone()),
        (branch.clone(), prompt.clone())
    );
    let (_, all, _) = kernel.session_tree(&session).unwrap();
    assert!(all.iter().any(|n| n.id == reply));
    assert_eq!(
        kernel.branch_path(&branch).unwrap().0.last().unwrap().id,
        prompt
    );

    kernel.move_head(&branch, &reply, false).await.unwrap();
    let ids = |ns: &[Node]| ns.iter().map(|n| n.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&kernel.branch_path(&branch).unwrap().0), ids(&nodes));
}

/// The main line stays put while a background task of the part it would
/// leave runs (the task's note would land on a line that no longer holds its
/// call); a task from a call the moved line still holds does not hold it back.
#[tokio::test]
async fn moving_the_main_line_waits_for_the_tasks_it_would_leave() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("started"),
            LlmResponse::text("second"),
        ],
    )
    .await;

    let (_, first, _) = run_turn(&kernel, &branch, "go").await;
    let (_, second, _) = run_turn(&kernel, &branch, "next").await;
    let prompt = |nodes: &[Node], text: &str| {
        nodes
            .iter()
            .find(|n| matches!(&n.body, NodeBody::User { content } if content.text == text))
            .expect("the turn's prompt")
            .id
            .clone()
    };
    let go = prompt(&first, "go");
    let next = prompt(&second, "next");
    let head = second.last().unwrap().id.clone();

    let err = kernel.move_head(&branch, &go, false).await.unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::LineBusy(_)), "{err:#}");
    assert!(err.to_string().contains("bgwork"), "{err:#}");
    assert_eq!(
        kernel.branch_path(&branch).unwrap().0.last().unwrap().id,
        head
    );
    kernel.move_head(&branch, &next, false).await.unwrap();
    assert_eq!(
        kernel.branch_path(&branch).unwrap().0.last().unwrap().id,
        next
    );
    kernel.abort_tasks(&branch, true, rumu_kernel::CancelSource::User);
}

/// With `stop`, the move goes through and stops the task it leaves behind:
/// the task ends cancelled, leaves no note on the new line and wakes nothing.
#[tokio::test]
async fn moving_the_main_line_with_stop_stops_the_tasks_it_leaves() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("started"),
        ],
    )
    .await;

    let (_, first, _) = run_turn(&kernel, &branch, "go").await;
    let go = first
        .iter()
        .find(|n| matches!(&n.body, NodeBody::User { .. }))
        .expect("the turn's prompt")
        .id
        .clone();
    let id = kernel.tasks(&session).unwrap()[0].id.clone();

    kernel.move_head(&branch, &go, true).await.unwrap();
    let mut status = TaskStatus::Running;
    for _ in 0..100 {
        status = kernel.task(&id).unwrap().unwrap().0.status;
        if !status.is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(status, TaskStatus::Cancelled);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(nodes.last().unwrap().id, go);
    assert!(kernel.is_idle(&branch).unwrap());
}

/// `branch.move` refuses a sub-agent's branch and a node a sub-agent added,
/// before anything changes.
#[tokio::test]
async fn move_refuses_sub_agents_and_their_nodes() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, main) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({"mode": "fork", "background": false, "task": "dig"}),
            )]),
            LlmResponse::text("child summary"),
            LlmResponse::text("parent done"),
        ],
    )
    .await;
    let (_, nodes, _) = run_turn_on(&kernel, &main, "go").await;
    let head = nodes.last().unwrap().id.clone();

    let tree = || kernel.session_tree(&session).unwrap();
    let status = |id: &BranchId| {
        tree()
            .2
            .iter()
            .find(|b| &b.id == id)
            .unwrap_or_else(|| panic!("branch {id} missing"))
            .status
    };
    let child = tree()
        .2
        .into_iter()
        .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { .. }))
        .expect("agent branch exists");

    // The sub-agent's own branch is not a main line.
    let err = kernel
        .move_head(&child.id, &child.head, false)
        .await
        .unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::NotMainLine(_)), "{err:#}");
    // Nor is the main line moved onto the sub-agent's reply.
    let err = kernel
        .move_head(&main, &child.head, false)
        .await
        .unwrap_err();
    assert!(matches!(err, rumu_kernel::Error::OffTheLine(_)), "{err:#}");
    assert_eq!(
        kernel.branch_path(&main).unwrap().0.last().unwrap().id,
        head
    );
    assert_eq!(status(&child.id), BranchStatus::Done);
}

/// `session.fork` copies the root→node path into a new session: fresh node
/// ids with parents remapped and bodies unchanged (plugin invoke nodes
/// included), sub-agent branches left behind, and the source session
/// untouched. An assistant node whose tool calls have no safe point is
/// refused.
#[tokio::test]
async fn fork_session_copies_the_path_into_a_new_session() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["plus"],
        vec![],
        noop_hooks(),
        plus_tool(),
    ));
    let store = Arc::new(Store::in_memory());
    let kernel = Kernel::new(
        store.clone(),
        Arc::new(FakeLlm::new(vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "plus", json!({"a": 2, "b": 3}))]),
            LlmResponse::text("done"),
        ])),
        vec![plugin],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    let (_, nodes, _) = run_turn_on(&kernel, &branch, "hello").await;
    let result = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::ToolResult { .. }))
        .expect("the turn appended a tool result")
        .id
        .clone();

    // A plugin invoke under the on-path tool call: forking at it copies the
    // path up to it. A sub-agent branch must stay behind.
    let invoke = Node {
        id: NodeId::new(),
        parent: Some(result.clone()),
        session: session.id.clone(),
        at: rumu_core::now(),
        dur_ms: Some(3),
        body: NodeBody::ToolResult {
            call_id: "c1.1".into(),
            content: Content::text("5"),
            is_error: false,
            invoke: Some(Invoke {
                parent: "c1".into(),
                name: "plus".into(),
                args: json!({"a": 2, "b": 3}),
                blocked: None,
            }),
        },
    };
    store.insert_node(&invoke).unwrap();
    let rpos = nodes.iter().position(|n| n.id == result).unwrap();
    let mut path: Vec<&Node> = nodes[..=rpos].iter().collect();
    path.push(&invoke);
    assert_eq!(
        path.len(),
        5,
        "root, user, assistant+calls, tool result, invoke"
    );
    let main = store.get_branch(&session.main_branch).unwrap().unwrap();
    store
        .insert_branch(&rumu_core::Branch {
            id: BranchId::new(),
            session: session.id.clone(),
            head: main.head.clone(),
            forked_from: Some(main.head.clone()),
            kind: rumu_core::BranchKind::Agent {
                task: "work".into(),
                parent_branch: main.id.clone(),
                call_id: "c1".into(),
                via_plugin: false,
                inherit: Default::default(),
            },
            status: BranchStatus::Done,
            created_at: rumu_core::now(),
        })
        .unwrap();

    let forked = kernel.fork_session(&invoke.id).unwrap();
    assert_ne!(forked.id, session.id);
    assert_eq!(forked.title, "hello", "the source session's title");

    // The copied path: same bodies and timestamps, fresh ids, parents
    // remapped into a single chain rooted at the copy of the root.
    // (`session_tree` orders by `at`, ambiguous within one millisecond, so
    // the new path is walked head→root like everywhere else.)
    let (info, new_nodes, new_branches) = kernel.session_tree(&forked.id).unwrap();
    assert_eq!(new_nodes.len(), path.len());
    let by_id: HashMap<&str, &Node> = new_nodes.iter().map(|n| (n.id.0.as_str(), n)).collect();
    let old_ids: HashSet<&str> = path.iter().map(|n| n.id.0.as_str()).collect();
    assert!(
        new_nodes.iter().all(|n| !old_ids.contains(n.id.0.as_str())),
        "every copied node has a fresh id"
    );
    assert_eq!(new_branches.len(), 1);
    let new_branch = &new_branches[0];
    let mut chain: Vec<&Node> = vec![];
    let mut cur = Some(new_branch.head.0.as_str());
    while let Some(id) = cur {
        let n = by_id[id];
        cur = n.parent.as_ref().map(|p| p.0.as_str());
        chain.push(n);
    }
    chain.reverse();
    for (i, n) in chain.iter().enumerate() {
        assert_eq!(n.session, forked.id);
        assert_eq!(
            serde_json::to_value(&n.body).unwrap(),
            serde_json::to_value(&path[i].body).unwrap()
        );
        assert_eq!(n.at, path[i].at);
        assert_eq!(n.dur_ms, path[i].dur_ms);
        assert_eq!(
            n.parent.as_ref(),
            i.checked_sub(1).map(|j| chain[j].id.clone()).as_ref()
        );
    }
    // One plain branch heading at the copied node; the agent branch stayed.
    assert!(matches!(new_branch.kind, rumu_core::BranchKind::Main));
    assert_eq!(new_branch.head, chain.last().unwrap().id);
    assert_eq!(info.main_branch, new_branch.id);

    // The source session is untouched.
    let (src_info, src_nodes, src_branches) = kernel.session_tree(&session.id).unwrap();
    assert_eq!(src_info.main_branch, session.main_branch);
    assert_eq!(src_nodes.len(), path.len() + 1);
    assert_eq!(src_branches.len(), 2);

    // Forking at an assistant node with tool calls is refused.
    let with_calls = nodes
        .iter()
        .find(
            |n| matches!(&n.body, NodeBody::Assistant { tool_calls, .. } if !tool_calls.is_empty()),
        )
        .expect("the turn made a tool call")
        .id
        .clone();
    let err = kernel.fork_session(&with_calls).unwrap_err();
    assert!(
        matches!(err, rumu_kernel::Error::UnsafeBranchPoint),
        "{err:#}"
    );
}

// ---------- out-of-band posts ----------
/// A post carrying one plain note.
fn note_post(text: &str, wake: bool) -> Post {
    Post {
        effects: vec![Content::text(text)],
        wake,
        paced: false,
    }
}

/// Wait for the next push satisfying `pred`.
async fn wait_push(rx: &mut PushRx, pred: impl Fn(&Value) -> bool, label: &str) -> Value {
    loop {
        let v = rx
            .recv()
            .await
            .unwrap_or_else(|| panic!("push stream ended before {label}"));
        if pred(&v) {
            return v;
        }
    }
}

/// Collect pushes until a `turn.done`/`turn.error` for `branch`.
async fn collect_turn(rx: &mut PushRx, branch: &str) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    loop {
        let v = rx.recv().await.expect("push stream ended");
        let m = v["method"].as_str().unwrap_or_default();
        if (m == "turn.done" || m == "turn.error") && v["params"]["branch"] == branch {
            return (v, seen);
        }
        seen.push(v);
    }
}

fn pushed_note_text(v: &Value) -> Option<&str> {
    (v["method"] == "node.appended").then(|| {
        let body = &v["params"]["node"]["body"];
        (body["type"] == "note")
            .then(|| body["content"]["text"].as_str())
            .flatten()
    })?
}

fn turn_starts(seen: &[Value], reason: &str) -> usize {
    seen.iter()
        .filter(|v| v["method"] == "turn.started" && v["params"]["reason"].as_str() == Some(reason))
        .count()
}

/// A wake post on an idle branch starts a turn; without `wake` it just writes.
#[tokio::test]
async fn post_to_idle_branch_with_and_without_wake() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &[],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) =
        setup(vec![plugin.clone()], vec![LlmResponse::text("woke up")]).await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    plugin
        .post(&branch, note_post("quiet", false))
        .expect("post");
    wait_push(
        &mut rx,
        |v| pushed_note_text(v) == Some("quiet"),
        "quiet note",
    )
    .await;
    // Give a wrongly-scheduled turn time to appear.
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(llm.projections().len(), 0, "no turn without wake");

    plugin.post(&branch, note_post("loud", true)).expect("post");
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 1);
    let started = seen
        .iter()
        .find(|v| v["method"] == "turn.started")
        .expect("turn started");
    assert_eq!(started["params"]["sources"], json!(["poster"]));
    assert!(seen.iter().any(|v| pushed_note_text(v) == Some("loud")));
    assert_eq!(llm.projections().len(), 1);
}

/// A post while a turn is running is queued and flushed before the next LLM
/// call, so that call already sees it — and no extra turn is started.
#[tokio::test]
async fn post_while_busy_lands_at_the_next_safe_point() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &["nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "nap", json!({"ms": 200}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "go").await.unwrap();
    // Wait until the assistant node is on the path: the nap tool is running.
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    plugin
        .post(&branch, note_post("mid-turn", true))
        .expect("post");

    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 0, "no extra turn");
    let projections = llm.projections();
    assert_eq!(projections.len(), 2);
    assert!(
        projections[1].iter().any(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "mid-turn")
        ),
        "the running turn saw the post"
    );
}

/// Two wake posts inside the debounce window write both notes and start one
/// turn.
#[tokio::test]
async fn coalescing_two_wakes_into_one_turn() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &[],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) =
        setup(vec![plugin.clone()], vec![LlmResponse::text("combined")]).await;
    kernel.set_wake_debounce(Duration::from_millis(60));
    let mut rx = kernel.tap_pushes();

    plugin
        .post(&branch, note_post("first", true))
        .expect("post");
    plugin
        .post(&branch, note_post("second", true))
        .expect("post");
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 1, "one turn for both wakes");
    let notes = seen
        .iter()
        .filter_map(pushed_note_text)
        .filter(|t| *t == "first" || *t == "second")
        .count();
    assert_eq!(notes, 2);
    assert_eq!(llm.projections().len(), 1);

    // No second turn starts afterwards.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while let Ok(v) = rx.try_recv() {
        assert_ne!(v["method"], "turn.started", "unexpected extra turn: {v}");
    }
}

/// Abort drops the wake flag of already-queued items; their content is still
/// written, but no new turn starts.
#[tokio::test]
async fn abort_drops_queued_wake_but_writes_content() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &["nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "nap", json!({"ms": 500}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(500));
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "go").await.unwrap();
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    plugin
        .post(&branch, note_post("queued", true))
        .expect("post");
    // Let the post worker enqueue it, then abort.
    tokio::time::sleep(Duration::from_millis(40)).await;
    kernel.abort(&branch).unwrap();

    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 0);
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(
        nodes
            .iter()
            .any(|n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "queued")),
        "content is still written"
    );
}

/// After many consecutive wake-started turns the guard writes content but
/// stops waking and warns.
#[tokio::test]
async fn storm_guard_stops_runaway_wakes() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &[],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let turns = rumu_kernel::MAX_WAKE_TURNS;
    let script = (0..turns)
        .map(|_| LlmResponse::text("ok"))
        .collect::<Vec<_>>();
    let (kernel, _llm, _s, branch) = setup(vec![plugin.clone()], script).await;
    kernel.set_wake_debounce(Duration::from_millis(5));
    let mut rx = kernel.tap_pushes();

    for i in 0..turns {
        plugin
            .post(&branch, note_post(&format!("wake-{i}"), true))
            .expect("post");
        let (done, _) = collect_turn(&mut rx, &branch.0).await;
        assert_eq!(done["method"], "turn.done");
    }

    plugin
        .post(&branch, note_post("too-many", true))
        .expect("post");
    let warning = wait_push(&mut rx, |v| v["method"] == "warning", "storm warning").await;
    assert!(
        warning["params"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("wake storm")),
        "{warning}"
    );
    // The content still lands and no turn starts.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(
        nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "too-many")
        )
    );
    while let Ok(v) = rx.try_recv() {
        assert_ne!(v["method"], "turn.started", "unexpected turn: {v}");
    }
}

/// Paced posts (a poster with its own rate limit and lifetime) wake past the
/// storm guard's budget, and do not use it up for other wakes.
#[tokio::test]
async fn paced_wakes_skip_the_storm_guard() {
    let plugin = Arc::new(plugin_with(
        "poster",
        &[],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let turns = rumu_kernel::MAX_WAKE_TURNS + 2;
    let script = (0..turns + 1)
        .map(|_| LlmResponse::text("ok"))
        .collect::<Vec<_>>();
    let (kernel, _llm, _s, branch) = setup(vec![plugin.clone()], script).await;
    kernel.set_wake_debounce(Duration::from_millis(5));
    let mut rx = kernel.tap_pushes();

    for i in 0..turns {
        let mut post = note_post(&format!("event-{i}"), true);
        post.paced = true;
        plugin.post(&branch, post).expect("post");
        let (done, _) = collect_turn(&mut rx, &branch.0).await;
        assert_eq!(done["method"], "turn.done");
    }
    // An ordinary wake still has the whole budget.
    plugin
        .post(&branch, note_post("plain", true))
        .expect("post");
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    while let Ok(v) = rx.try_recv() {
        assert_ne!(v["method"], "warning", "unexpected warning: {v}");
    }
}

/// The `post` event is interceptable: Block drops the post, Rewrite patches
/// its effects, and `source` is matchable.
#[tokio::test]
async fn post_event_block_and_rewrite() {
    let blocker = Arc::new(plugin_with(
        "poster",
        &[],
        vec![hook("guard", json!({"event": "post", "source": "poster"}))],
        block_hook("no posts"),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(vec![blocker.clone()], vec![]).await;
    blocker
        .post(&branch, note_post("dropped", false))
        .expect("post");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(
        !nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "dropped")
        ),
        "blocked post was dropped"
    );

    let rewriter = Arc::new(plugin_with(
        "poster",
        &[],
        vec![hook("shim", json!({"event": "post", "source": "poster"}))],
        Arc::new(|_, _| rewrite(json!({"effects": [{"text": "patched"}]}))),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(vec![rewriter.clone()], vec![]).await;
    rewriter
        .post(&branch, note_post("original", false))
        .expect("post");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(
        nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "patched")
        ),
        "rewrite patched the effects"
    );
    assert!(
        !nodes.iter().any(
            |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "original")
        )
    );
}

/// Steering: a prompt while a turn is running is queued as a wake item and
/// seen by the running turn's next LLM call (no extra turn).
#[tokio::test]
async fn steering_prompt_while_busy_is_seen_by_the_running_turn() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "nap", json!({"ms": 250}))]),
            LlmResponse::text("first done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "first").await.unwrap();
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    kernel.prompt(&branch, "steer").await.unwrap();
    wait_push(&mut rx, |v| v["method"] == "prompt.queued", "prompt.queued").await;

    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 0);
    let projections = llm.projections();
    assert_eq!(projections.len(), 2);
    let users: Vec<&str> = projections[1]
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::User { content } => Some(content.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["first", "steer"]);
}

/// A queued steering prompt taken back before the safe point is never
/// written and wakes nothing; once written it can no longer be cancelled.
#[tokio::test]
async fn cancelled_steering_prompt_is_never_written() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "nap", json!({"ms": 250}))]),
            LlmResponse::text("first done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "first").await.unwrap();
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    kernel.prompt(&branch, "steer").await.unwrap();
    let queued = wait_push(&mut rx, |v| v["method"] == "prompt.queued", "prompt.queued").await;
    let node = NodeId::from(queued["params"]["node"].as_str().unwrap());
    let content = kernel.cancel_queued(&branch, &node).unwrap();
    assert_eq!(content.text, "steer");
    assert!(matches!(
        kernel.cancel_queued(&branch, &node),
        Err(rumu_kernel::Error::NotQueued)
    ));

    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let projections = llm.projections();
    assert_eq!(
        projections.len(),
        2,
        "no wake turn for the cancelled prompt"
    );
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(
        !nodes
            .iter()
            .any(|n| matches!(&n.body, NodeBody::User { content } if content.text == "steer"))
    );
}

/// A stop hands the user's steering prompts back unwritten, wakes nothing for
/// them, and still writes another agent's message (without a turn).
#[tokio::test]
async fn stop_hands_back_steering_prompts() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin],
        vec![LlmResponse::calls(vec![ToolCall::new(
            "c1",
            "nap",
            json!({"ms": 250}),
        )])],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "first").await.unwrap();
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    kernel.prompt(&branch, "steer").await.unwrap();
    let queued = wait_push(&mut rx, |v| v["method"] == "prompt.queued", "prompt.queued").await;
    kernel
        .send(&branch, Content::text("from an agent"))
        .await
        .unwrap();
    wait_push(&mut rx, |v| v["method"] == "prompt.queued", "prompt.queued").await;

    let back = kernel.stop(&branch).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].0.0, queued["params"]["node"].as_str().unwrap());
    assert_eq!(back[0].1.text, "steer");

    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(llm.projections().len(), 1, "no turn after the stop");
    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let users: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::User { content } => Some(content.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["first", "from an agent"]);
}

/// A gated fake LLM: signals each call and waits for a permit, so a test can
/// inject a prompt exactly while the final call of a turn is in flight.
struct GatedLlm {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: tokio::sync::Semaphore,
    inner: ScriptedLlm,
}

#[async_trait]
impl Llm for GatedLlm {
    async fn complete(
        &self,
        pick: rumu_core::ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        let _ = self.started.send(());
        let permit = self.release.acquire().await.expect("semaphore open");
        permit.forget();
        self.inner
            .complete(pick, system, tools, nodes, on_delta)
            .await
    }
}

/// If the running turn ends before the queued steering message is flushed, it
/// starts a new turn instead.
#[tokio::test]
async fn steering_prompt_starts_a_new_turn_if_the_turn_ended_first() {
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let llm = Arc::new(GatedLlm {
        started: started_tx,
        release: tokio::sync::Semaphore::new(0),
        inner: ScriptedLlm::strict(vec![
            LlmResponse::text("first reply"),
            LlmResponse::text("second reply"),
        ]),
    });
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        llm.clone(),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    kernel.set_wake_debounce(Duration::from_millis(5));
    let mut rx = kernel.tap_pushes();

    kernel.prompt(&branch, "first").await.unwrap();
    // The first LLM call is in flight.
    started_rx.recv().await.expect("first LLM call");
    // Steering arrives while busy; the call then finishes without tools.
    kernel.prompt(&branch, "steer").await.unwrap();
    llm.release.add_permits(1);

    // The queued user message starts a second turn.
    tokio::time::timeout(Duration::from_secs(2), started_rx.recv())
        .await
        .expect("second LLM call")
        .expect("started");
    llm.release.add_permits(1);

    let (done1, seen1) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done1["method"], "turn.done");
    let (done2, seen2) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done2["method"], "turn.done");
    assert_eq!(turn_starts(&seen1, "prompt"), 1);
    assert_eq!(turn_starts(&seen2, "prompt"), 1);

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let users: Vec<&str> = nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::User { content } => Some(content.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["first", "steer"]);
}

/// `Cx::invoke` recursion is bounded.
#[tokio::test]
async fn invoke_depth_limit_is_enforced() {
    let plugin = Arc::new(plugin_with(
        "deep",
        &["recurse"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin.clone()],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "recurse", json!({}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let (text, is_error) = result_text(&nodes, "c1");
    assert!(is_error, "{text}");
    assert!(text.contains("invoke depth limit"), "{text}");
    // Direct call + four nested invokes reach the plugin; the fifth is rejected.
    let calls = plugin
        .calls()
        .iter()
        .filter(|(name, _)| name == "recurse")
        .count();
    assert_eq!(calls, rumu_kernel::MAX_INVOKE_DEPTH + 1);
}

/// A wake-started turn on a finished agent branch re-activates it, runs until
/// a final reply, and closes it out again.
#[tokio::test]
async fn agent_wake_turn_runs_to_a_final_reply() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent", "other"],
        vec![],
        noop_hooks(),
        Arc::new(|_, _, _| ToolOutput::ok("other-done")),
    ));
    let script = vec![
        LlmResponse::calls(vec![ToolCall::new(
            "e1",
            "agent",
            json!({ "mode": "fork", "background": false, "task": "look" }),
        )]),
        LlmResponse::text("child summary"),
        LlmResponse::text("parent done"),
        // The wake turn on the child: one tool call, then the final reply.
        LlmResponse::calls(vec![ToolCall::new("c", "other", json!({}))]),
        LlmResponse::text("woken reply"),
    ];
    let (kernel, _llm, session, branch) = setup(vec![plugin.clone()], script).await;
    run_turn_on(&kernel, &branch, "go").await;

    let child = agent_branches(&kernel, &session)
        .into_iter()
        .next()
        .expect("agent child");
    assert_eq!(child.status, BranchStatus::Done);

    let mut rx = kernel.tap_pushes();
    plugin
        .post(&child.id, note_post("afterwards", true))
        .expect("post");
    let started = wait_push(
        &mut rx,
        |v| v["method"] == "turn.started" && v["params"]["branch"] == child.id.0,
        "wake turn on the child",
    )
    .await;
    assert_eq!(started["params"]["reason"], "wake");
    let (done, _) = collect_turn(&mut rx, &child.id.0).await;
    assert_eq!(done["method"], "turn.done");

    let (nodes, _) = kernel.branch_path(&child.id).unwrap();
    assert!(nodes.iter().any(
        |n| matches!(&n.body, NodeBody::Note { content, .. } if content.text == "afterwards")
    ));
    let tool_call_steps = nodes
        .iter()
        .filter(|n| {
            matches!(&n.body, NodeBody::Assistant { tool_calls, .. }
                if tool_calls.iter().any(|c| c.name == "other"))
        })
        .count();
    assert_eq!(tool_call_steps, 1, "the wake turn ran its scripted step");
    assert!(
        nodes.iter().any(|n| matches!(
            &n.body,
            NodeBody::Assistant { content, .. } if content.text.contains("woken reply")
        )),
        "the wake turn ended with its final reply"
    );
    assert_eq!(
        agent_branches(&kernel, &session)
            .into_iter()
            .find(|b| b.id == child.id)
            .expect("agent child")
            .status,
        BranchStatus::Done
    );
}

// ---------- background tasks ----------

/// A note push's structured tag, if any.
fn pushed_note_tag(v: &Value) -> Option<&str> {
    (v["method"] == "node.appended")
        .then(|| v["params"]["node"]["body"]["content"]["data"]["tag"].as_str())?
}

fn task_note_count(nodes: &[Node]) -> usize {
    nodes
        .iter()
        .filter(|n| {
            matches!(&n.body, NodeBody::Note { content, .. }
                if content.decode::<TaskDone>().is_some())
        })
        .count()
}

/// A scripted LLM with separate queues for the parent and for sub-agent
/// branches (recognised by the sub-agent framing), so background agents can
/// run concurrently without stealing the parent's replies.
struct SplitLlm {
    parent: ScriptedLlm,
    child: ScriptedLlm,
}

impl SplitLlm {
    fn new(parent: Vec<LlmResponse>, child: Vec<LlmResponse>) -> Self {
        Self {
            parent: ScriptedLlm::strict(parent),
            child: ScriptedLlm::strict(child),
        }
    }
}

#[async_trait]
impl Llm for SplitLlm {
    async fn complete(
        &self,
        pick: rumu_core::ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, rumu_kernel::LlmError> {
        let is_child = nodes.iter().any(|n| match &n.body {
            NodeBody::User { content } => content.text.contains("You are now a sub-agent"),
            _ => false,
        });
        let llm = if is_child { &self.child } else { &self.parent };
        llm.complete(pick, system, tools, nodes, on_delta).await
    }
}

async fn setup_split(
    plugins: Vec<Arc<dyn Plugin>>,
    parent: Vec<LlmResponse>,
    child: Vec<LlmResponse>,
) -> (Arc<Kernel>, SessionId, BranchId) {
    let store = Store::in_memory();
    let llm = Arc::new(SplitLlm::new(parent, child));
    let kernel = Kernel::new(Arc::new(store), llm, plugins, Workspaces::default()).unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    (kernel, session.id, branch)
}

/// A message to a finished sub-agent runs it again, and that turn's reply
/// reaches the parent as a note that wakes it — as its first reply did as the
/// `agent` result.
#[tokio::test]
async fn a_message_to_a_finished_sub_agent_reports_back_to_the_parent() {
    let (kernel, session, branch) = setup_split(
        vec![],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "a1",
                "agent",
                json!({ "background": false, "task": "look" }),
            )]),
            LlmResponse::text("parent done"),
            LlmResponse::text("parent read the follow-up"),
        ],
        vec![
            LlmResponse::text("child summary"),
            LlmResponse::text("child follow-up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    run_turn_on(&kernel, &branch, "go").await;
    let child = agent_branches(&kernel, &session).remove(0);
    assert_eq!(child.status, BranchStatus::Done);

    let mut rx = kernel.tap_pushes();
    kernel.prompt(&child.id, "one more thing").await.unwrap();
    let (done, _) = collect_turn(&mut rx, &child.id.0).await;
    assert_eq!(done["method"], "turn.done");
    let started = wait_push(
        &mut rx,
        |v| v["method"] == "turn.started" && v["params"]["branch"] == branch.0,
        "the parent's wake turn",
    )
    .await;
    assert_eq!(started["params"]["reason"], "wake");
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let note = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Note { content, .. } if content.text.starts_with("sub-agent \"look\"") => {
                Some(content.text.clone())
            }
            _ => None,
        })
        .expect("the follow-up reply on the parent");
    assert!(note.ends_with("child follow-up"), "{note}");
    assert!(nodes.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { content, .. } if content.text == "parent read the follow-up")));
    let child = agent_branches(&kernel, &session).remove(0);
    assert_eq!(child.status, BranchStatus::Done);
}

/// Once the main line has moved off the call that started a finished
/// sub-agent, a message still runs it, but its reply stays in its own branch:
/// the line it would reach never made that call.
#[tokio::test]
async fn a_sub_agent_the_main_line_moved_off_keeps_its_reply() {
    let (kernel, session, branch) = setup_split(
        vec![],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "a1",
                "agent",
                json!({ "background": false, "task": "look" }),
            )]),
            LlmResponse::text("parent done"),
        ],
        vec![
            LlmResponse::text("child summary"),
            LlmResponse::text("child follow-up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let (_, nodes, _) = run_turn_on(&kernel, &branch, "go").await;
    let child = agent_branches(&kernel, &session).remove(0);
    let prompt = nodes
        .iter()
        .find(|n| matches!(n.body, NodeBody::User { .. }))
        .expect("the prompt")
        .id
        .clone();
    kernel.move_head(&branch, &prompt, false).await.unwrap();

    let mut rx = kernel.tap_pushes();
    kernel.prompt(&child.id, "one more thing").await.unwrap();
    let (done, _) = collect_turn(&mut rx, &child.id.0).await;
    assert_eq!(done["method"], "turn.done");
    tokio::time::sleep(Duration::from_millis(50)).await;
    tokio::time::timeout(Duration::from_secs(5), kernel.wait_idle(&branch))
        .await
        .expect("the main line stays idle")
        .unwrap();

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert_eq!(
        nodes.last().unwrap().id,
        prompt,
        "nothing reached the main line"
    );
    let (own, _) = kernel.branch_path(&child.id).unwrap();
    assert!(own.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { content, .. } if content.text == "child follow-up")));
    let child = agent_branches(&kernel, &session).remove(0);
    assert_eq!(child.status, BranchStatus::Done);
}

/// A background tool returns a task id immediately; its completion note wakes
/// an idle branch with the result.
#[tokio::test]
async fn background_tool_returns_task_id_and_completion_wakes_idle_branch() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 100, "out": "all done", "background": true }),
            )]),
            LlmResponse::text("started"),
            LlmResponse::text("woken up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    let (done, nodes, _) = turn(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");
    let id = kernel.tasks(&session).unwrap()[0].id.clone();
    assert!(
        id.0.starts_with("t-") && id.0.len() == "t-00000000".len(),
        "{id}"
    );
    let (text, err) = result_text(&nodes, "c1");
    assert!(!err, "{text}");
    assert!(text.contains(&format!("started task {id}")), "{text}");
    assert!(text.contains("bgwork"), "{text}");

    // The completion note arrives and wakes the idle branch.
    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "task completion note",
    )
    .await;
    let note_text = pushed_note_text(&note).unwrap();
    assert!(
        note_text.contains(&format!("task {id} (bgwork ")),
        "{note_text}"
    );
    assert!(note_text.contains("finished: ok"), "{note_text}");
    assert!(note_text.contains("all done"), "{note_text}");
    let (done2, seen2) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done2["method"], "turn.done");
    assert_eq!(turn_starts(&seen2, "wake"), 1, "{seen2:?}");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    assert!(nodes.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { content, .. } if content.text == "woken up")));
    assert_eq!(task_note_count(&nodes), 1);

    let tasks = kernel.tasks(&session).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, id);
    assert_eq!(tasks[0].status, TaskStatus::Done);
    assert!(matches!(&tasks[0].kind, TaskKind::Tool { name, .. } if name == "bgwork"));
    assert_eq!(tasks[0].result.as_ref().unwrap().text, "all done");
}

/// A model's foreground call of a background-capable tool still running at
/// its `timeout` moves to the background: the result names the task and the
/// output so far, and the same work finishes there. One that ends in time is
/// an ordinary result; a script's call is stopped instead.
#[tokio::test]
async fn foreground_call_moves_to_the_background_at_its_timeout() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let host = Arc::new(script_host(&[], vec![], noop_hooks(), noop_tools()));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin, host],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new("quick", "bgwork", json!({ "ms": 10, "timeout": 5 })),
                ToolCall::new(
                    "slow",
                    "bgwork",
                    json!({ "ms": 400, "out": "all done", "timeout": 0.1 }),
                ),
                ToolCall::new(
                    "script",
                    "script",
                    json!({ "calls": [
                        { "name": "bgwork", "args": { "ms": 400, "timeout": 0.1 } }
                    ] }),
                ),
            ]),
            LlmResponse::text("moved"),
            LlmResponse::text("woken up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    let (_, nodes, _) = turn(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(
        result_text(&nodes, "quick"),
        ("finished work".into(), false)
    );
    let tasks = kernel.tasks(&session).unwrap();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    let id = tasks[0].id.clone();
    assert_eq!(tasks[0].call_id, "slow");
    let (text, err) = result_text(&nodes, "slow");
    assert!(!err, "{text}");
    assert!(
        text.contains(&format!(
            "still running after 0.1s, so it moved to the background as task {id}"
        )),
        "{text}"
    );
    assert!(
        text.contains("output so far:\nline one\nline two\n"),
        "{text}"
    );
    let (text, _) = result_text(&nodes, "script");
    assert!(text.contains("timed out after 0.1s"), "{text}");

    // The work goes on as the task: its buffer has the output from before
    // the move, and its completion note the result.
    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "task completion note",
    )
    .await;
    let note_text = pushed_note_text(&note).unwrap();
    assert!(note_text.contains("finished: ok\nall done"), "{note_text}");
    let (_, output) = kernel.task(&id).unwrap().unwrap();
    assert!(output.starts_with("line one\nline two\n"), "{output}");
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
}

/// A completion while the owner is busy is written at the next safe point and
/// seen by that turn's next LLM call (no extra wake turn).
#[tokio::test]
async fn background_completion_while_busy_lands_at_next_safe_point() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork", "nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 120, "background": true }),
            )]),
            LlmResponse::calls(vec![ToolCall::new("c2", "nap", json!({ "ms": 300 }))]),
            LlmResponse::text("saw the task"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();

    let (done, seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 0, "no extra wake turn");

    let projections = llm.projections();
    assert_eq!(projections.len(), 3);
    assert!(
        projections[2].iter().any(|n| matches!(&n.body,
            NodeBody::Note { content, .. } if content.decode::<TaskDone>().is_some())),
        "the safe point put the completion note before the next LLM call"
    );
}

/// `Cx::kill_task` cancels a running task and reports its status; the
/// cancelled task still posts a completion note naming the model.
#[tokio::test]
async fn kill_task_cancels_and_reports_cancelled() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork", "kill"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::calls(vec![ToolCall::new("c2", "kill", json!({ "id": "$task" }))]),
            LlmResponse::text("done"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let id = kernel.tasks(&session).unwrap()[0].id.clone();
    let (text, err) = result_text(&nodes, "c2");
    assert!(!err, "{text}");
    assert_eq!(text, format!("task {id}: cancelled"));
    let (note, note_text) = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Note { content, .. } => content
                .decode::<TaskDone>()
                .map(|d| (d, content.text.clone())),
            _ => None,
        })
        .expect("a cancelled-task completion note");
    assert_eq!(note.status, TaskStatus::Cancelled);
    assert!(
        note_text.contains("finished: cancelled by the model"),
        "{note_text}"
    );
    assert_eq!(
        kernel.tasks(&session).unwrap()[0].status,
        TaskStatus::Cancelled
    );
}

/// Aborting the owner cancels its background tasks and writes the note
/// without waking.
#[tokio::test]
async fn abort_cancels_owned_tasks_and_does_not_wake() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("started"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    wait_push(
        &mut rx,
        |v| v["method"] == "node.appended" && v["params"]["node"]["body"]["type"] == "assistant",
        "assistant node",
    )
    .await;
    wait_until(Duration::from_secs(5), || {
        kernel
            .tasks(&session)
            .unwrap()
            .iter()
            .any(|t| t.status.is_running())
    })
    .await;

    kernel.abort(&branch).unwrap();
    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "cancelled task note",
    )
    .await;
    assert!(
        pushed_note_text(&note)
            .unwrap()
            .contains("finished: cancelled by the user")
    );
    // The note did not start a wake turn.
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(v) = rx.try_recv() {
        assert_ne!(v["method"], "turn.started", "abort must not wake: {v}");
    }
    assert_eq!(
        kernel.tasks(&session).unwrap()[0].status,
        TaskStatus::Cancelled
    );
}

/// The UI's `task.kill` wakes the owner with the completion note, so the
/// model hears its task is gone.
#[tokio::test]
async fn ui_kill_wakes() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("started"),
            LlmResponse::text("noted"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    run_turn(&kernel, &branch, "go").await;
    let mut rx = kernel.tap_pushes();
    let id = kernel.tasks(&session).unwrap()[0].id.clone();

    let info = kernel.kill_task(&id).await.unwrap();
    assert_eq!(info.status, TaskStatus::Cancelled);
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 1, "{seen:?}");
    let note = seen
        .iter()
        .find(|v| pushed_note_tag(v) == Some("task.done"))
        .and_then(pushed_note_text)
        .expect("the completion note");
    assert!(note.contains("finished: cancelled by the user"), "{note}");
}

/// Killing a background sub-agent takes down the tasks it started, quietly,
/// so none of them is left to wake the dead child later.
#[tokio::test]
async fn killing_a_sub_agent_kills_its_tasks() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, session, branch) = setup_split(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "mode": "fork", "task": "watch it" }),
            )]),
            LlmResponse::text("parent done"),
            LlmResponse::text("parent heard"),
        ],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("waiting"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    let running = |kernel: &Kernel| {
        kernel
            .tasks(&session)
            .unwrap()
            .into_iter()
            .filter(|t| t.status.is_running())
            .collect::<Vec<_>>()
    };
    wait_until(Duration::from_secs(5), || running(&kernel).len() == 2).await;
    let agent = running(&kernel)
        .into_iter()
        .find(|t| matches!(t.kind, TaskKind::Agent { .. }))
        .unwrap();

    kernel.kill_task(&agent.id).await.unwrap();
    wait_until(Duration::from_secs(5), || running(&kernel).is_empty()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut seen = vec![];
    while let Ok(v) = rx.try_recv() {
        seen.push(v);
    }
    // The kill wakes the parent; nothing wakes the dead child.
    let wakes: Vec<_> = seen
        .iter()
        .filter(|v| v["method"] == "turn.started" && v["params"]["reason"] == "wake")
        .map(|v| v["params"]["branch"].clone())
        .collect();
    assert_eq!(wakes, vec![json!(branch.0)], "{seen:?}");
}

/// Running tasks per owner branch are capped.
#[tokio::test]
async fn per_branch_task_limit() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new("c1", "bgwork", json!({ "ms": 500, "background": true })),
                ToolCall::new("c2", "bgwork", json!({ "ms": 500, "background": true })),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    kernel.set_max_tasks_per_branch(1);

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let (text, err) = result_text(&nodes, "c2");
    assert!(err, "{text}");
    assert!(text.contains("limit 1"), "{text}");
}

/// `branch.stop` ends the reply and leaves the background task it started
/// running (`abort`, which takes it down too, is
/// `abort_cancels_owned_tasks_and_does_not_wake`).
#[tokio::test]
async fn stop_ends_the_reply_and_keeps_its_tasks() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": true }),
            )]),
            LlmResponse::text("started"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    wait_until(Duration::from_secs(5), || {
        kernel
            .tasks(&session)
            .unwrap()
            .iter()
            .any(|t| t.status.is_running())
    })
    .await;

    kernel.stop(&branch).unwrap();
    wait_until(Duration::from_secs(5), || {
        kernel.is_idle(&branch).unwrap_or(false)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        kernel.tasks(&session).unwrap()[0].status.is_running(),
        "a stopped reply leaves its background task alone"
    );
    while let Ok(v) = rx.try_recv() {
        assert_ne!(v["method"], "task.done", "the task must not finish: {v}");
    }
    kernel.abort_tasks(&branch, true, rumu_kernel::CancelSource::User);
}

// ---------- restart ----------

/// A task still running when its kernel stopped comes back as `lost` in the
/// next kernel's table, with the completion note it never posted — and only
/// the first restart reports it.
#[tokio::test]
async fn restart_reports_a_lost_task_once() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let seen = dir.path().join("seen.json");
    let store = Arc::new(Store::open(&sessions, &seen).unwrap());
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let llm = Arc::new(FakeLlm::new(vec![
        LlmResponse::calls(vec![ToolCall::new(
            "c1",
            "bgwork",
            json!({ "ms": 60_000, "background": true }),
        )]),
        LlmResponse::text("started"),
    ]));
    let kernel = Kernel::new(store.clone(), llm, vec![plugin], Workspaces::default()).unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    run_turn(&kernel, &branch, "go").await;
    let id = kernel.tasks(&session.id).unwrap()[0].id.clone();
    assert!(kernel.tasks(&session.id).unwrap()[0].status.is_running());

    // A second kernel over the same journals: the task died with the first.
    let second = Kernel::new(
        store.clone(),
        Arc::new(FakeLlm::new(vec![])),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    wait_until(Duration::from_secs(5), || {
        second
            .tasks(&session.id)
            .is_ok_and(|t| t.iter().any(|t| t.id == id && t.status == TaskStatus::Lost))
    })
    .await;
    let adopted = second.tasks(&session.id).unwrap();
    let task = adopted.iter().find(|t| t.id == id).unwrap();
    assert!(task.is_error, "a lost task reads as an error");
    assert!(task.ended_at.is_some());
    let notes = |k: &Arc<Kernel>| -> Vec<String> {
        k.branch_path(&branch)
            .unwrap()
            .0
            .iter()
            .filter_map(|n| match &n.body {
                NodeBody::Note { content, .. }
                    if content
                        .decode::<TaskDone>()
                        .is_some_and(|d| d.status == TaskStatus::Lost) =>
                {
                    Some(content.text.clone())
                }
                _ => None,
            })
            .collect()
    };
    let reported = notes(&second);
    assert_eq!(reported.len(), 1, "{reported:?}");
    assert!(
        reported[0].starts_with(&format!("task {id} (bgwork "))
            && reported[0].contains("finished: lost"),
        "{}",
        reported[0]
    );

    // The record says it is over now: a third kernel adopts it silently.
    let third = Kernel::new(
        store,
        Arc::new(FakeLlm::new(vec![])),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    wait_until(Duration::from_secs(5), || {
        third
            .tasks(&session.id)
            .is_ok_and(|t| t.iter().any(|t| t.id == id && t.status == TaskStatus::Lost))
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(notes(&third).len(), 1, "reported once");
}

/// A plugin invoke (`code` script) with `background` starts a task: the
/// script gets its id, and the result comes later as a note that wakes the
/// branch.
#[tokio::test]
async fn background_from_plugin_invoke_starts_a_task() {
    let host = Arc::new(script_host(&[], vec![], noop_hooks(), noop_tools()));
    let bg = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![host, bg],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "script",
                json!({ "calls": [{ "name": "bgwork", "args": { "ms": 50, "out": "all done", "background": true } }] }),
            )]),
            LlmResponse::text("done"),
            LlmResponse::text("woken up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    let (_, nodes, _) = turn(&kernel, &branch, &mut rx, "go").await;
    let tasks = kernel.tasks(&session).unwrap();
    assert_eq!(tasks.len(), 1);
    let id = tasks[0].id.clone();
    assert_eq!(tasks[0].call_id, "c1", "hangs from the script's call");
    let (text, _) = result_text(&nodes, "c1");
    let rows: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(rows[0]["ok"], true, "{text}");
    assert!(
        rows[0]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("started task {id}")),
        "{text}"
    );

    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "task completion note",
    )
    .await;
    let note_text = pushed_note_text(&note).unwrap();
    assert!(note_text.contains("all done"), "{note_text}");
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done");
    assert_eq!(turn_starts(&seen, "wake"), 1, "{seen:?}");
}

/// A background task cannot make internal calls: `Cx::invoke` refuses.
#[tokio::test]
async fn background_tool_cannot_invoke() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["try_invoke"],
        vec![],
        noop_hooks(),
        plus_tool(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "try_invoke",
                json!({ "background": true }),
            )]),
            LlmResponse::text("started"),
            LlmResponse::text("woken"),
        ],
    )
    .await;

    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let (text, err) = result_text(&nodes, "c1");
    assert!(!err, "{text}");
    assert!(text.contains("started task "), "{text}");

    wait_until(Duration::from_secs(5), || {
        kernel
            .tasks(&session)
            .unwrap()
            .first()
            .is_some_and(|t| !t.status.is_running())
    })
    .await;
    let tasks = kernel.tasks(&session).unwrap();
    let result = tasks[0].result.as_ref().expect("task result");
    assert!(
        result
            .text
            .contains("invoke is not available in a background task"),
        "{}",
        result.text
    );
}

/// An `agent` call runs in the background by default and posts its summary
/// to the parent when it finishes.
#[tokio::test]
async fn agent_runs_in_background_by_default() {
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, session, branch) = setup_split(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "mode": "fork", "task": "find it" }),
            )]),
            LlmResponse::text("parent done"),
            LlmResponse::text("parent woke"),
        ],
        vec![LlmResponse::text("child summary")],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    let (done, seen) = turn_collect(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");
    // The agent completed as a task later; its note wakes the parent.
    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "agent task note",
    )
    .await;
    let text = pushed_note_text(&note).unwrap();
    let id = kernel.tasks(&session).unwrap()[0].id.clone();
    assert!(
        text.contains(&format!("task {id} (agent \"find it\") finished: ok")),
        "{text}"
    );
    assert!(text.contains("child summary"), "{text}");
    let (done2, seen2) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done2["method"], "turn.done");
    assert_eq!(turn_starts(&seen2, "wake"), 1, "{seen:?}");

    let tasks = kernel.tasks(&session).unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, TaskStatus::Done);
    match &tasks[0].kind {
        TaskKind::Agent { child_branch } => {
            let child = child_branch.as_ref().expect("child branch recorded");
            let done = kernel
                .session_tree(&session)
                .unwrap()
                .2
                .into_iter()
                .find(|b| &b.id == child)
                .expect("the child branch exists");
            assert_eq!(done.status, BranchStatus::Done);
        }
        other => panic!("expected an agent task, got {other:?}"),
    }
}

/// Stopping a background sub-agent's own branch reports the task as
/// cancelled (not ok), says the user interrupted it, and still wakes the
/// parent with the partial summary.
#[tokio::test]
async fn user_abort_of_agent_branch_reports_cancelled() {
    let agent = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, session, branch) = setup_split(
        vec![agent, plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "mode": "fork", "task": "find it" }),
            )]),
            LlmResponse::text("parent done"),
            LlmResponse::text("parent woke"),
        ],
        vec![LlmResponse {
            text: "looking".into(),
            ..LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 60_000, "background": false }),
            )])
        }],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();
    let (done, _) = turn_collect(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");

    let child = wait_push(
        &mut rx,
        |v| {
            v["method"] == "node.appended"
                && v["params"]["branch"] != branch.0
                && v["params"]["node"]["body"]["type"] == "assistant"
        },
        "child assistant node",
    )
    .await;
    let child = BranchId::from(child["params"]["branch"].as_str().unwrap());
    kernel.abort(&child).unwrap();

    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "agent task note",
    )
    .await;
    let text = pushed_note_text(&note).unwrap();
    assert!(text.contains("finished: cancelled"), "{text}");
    assert!(text.contains("interrupted by the user"), "{text}");
    assert!(text.contains("looking"), "{text}");
    assert_eq!(
        kernel.tasks(&session).unwrap()[0].status,
        TaskStatus::Cancelled
    );
    let (done2, seen2) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done2["method"], "turn.done");
    assert_eq!(turn_starts(&seen2, "wake"), 1, "{seen2:?}");
}

/// Stopping a sub-agent stops it as the main line stops: its background tasks
/// run on, and when one finishes it wakes the sub-agent, whose reply reaches
/// the parent as a note.
#[tokio::test]
async fn a_stopped_sub_agent_keeps_its_tasks() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, session, branch) = setup_split(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "a1",
                "agent",
                json!({ "background": true, "task": "dig" }),
            )]),
            LlmResponse::text("parent done"),
            LlmResponse::text("parent saw the interruption"),
            LlmResponse::text("parent read the reply"),
        ],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 400, "background": true }),
            )]),
            LlmResponse::text("waiting"),
            LlmResponse::text("child woke"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();

    let waiting = wait_push(
        &mut rx,
        |v| {
            v["method"] == "node.appended"
                && v["params"]["node"]["body"]["content"]["text"] == "waiting"
        },
        "the child waiting on its task",
    )
    .await;
    let child = BranchId::from(waiting["params"]["branch"].as_str().unwrap());
    kernel.stop(&child).unwrap();

    let note = wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "agent task note",
    )
    .await;
    let text = pushed_note_text(&note).unwrap();
    assert!(text.contains("keep running"), "{text}");

    wait_until(Duration::from_secs(5), || {
        kernel.branch_path(&branch).unwrap().0.iter().any(|n| {
            matches!(&n.body, NodeBody::Note { content, .. }
                if content.text.contains("replied:\nchild woke"))
        })
    })
    .await;
    let work = kernel
        .tasks(&session)
        .unwrap()
        .into_iter()
        .find(|t| t.branch == child)
        .expect("the child's task");
    assert_eq!(work.status, TaskStatus::Done);
}

/// A scripted parent whose sub-agents' model calls all fail.
struct ChildFails {
    parent: ScriptedLlm,
}

#[async_trait]
impl Llm for ChildFails {
    async fn complete(
        &self,
        pick: rumu_core::ModelPick<'_>,
        system: &str,
        tools: &[ToolSpec],
        nodes: &[Node],
        on_delta: Option<OnDelta>,
    ) -> std::result::Result<LlmResponse, LlmError> {
        let is_child = nodes.iter().any(|n| match &n.body {
            NodeBody::User { content } => content.text.contains("You are now a sub-agent"),
            _ => false,
        });
        if is_child {
            return Err(LlmError::Request("400 bad request".into()));
        }
        self.parent
            .complete(pick, system, tools, nodes, on_delta)
            .await
    }
}

/// A sub-agent whose loop fails releases its branch like a failed turn: Done,
/// idle, the failure pushed — not left busy with nothing to stop.
#[tokio::test]
async fn failed_sub_agent_releases_its_branch() {
    let store = Arc::new(Store::in_memory());
    let llm = Arc::new(ChildFails {
        parent: ScriptedLlm::looping(vec![
            LlmResponse::calls(vec![ToolCall::new(
                "a1",
                "agent",
                json!({ "task": "look" }),
            )]),
            LlmResponse::text("parent done"),
        ]),
    });
    let kernel = Kernel::new(store.clone(), llm, vec![], Workspaces::default()).unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = BranchId::from(session.main_branch.0.as_str());
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();

    let error = wait_push(
        &mut rx,
        |v| v["method"] == "turn.error" && v["params"]["branch"] != branch.0,
        "the child's turn.error",
    )
    .await;
    let child = BranchId::from(error["params"]["branch"].as_str().unwrap());
    assert!(kernel.is_idle(&child).unwrap());
    let b = store.get_branch(&child).unwrap().unwrap();
    assert_eq!(b.status, BranchStatus::Done);
    wait_until(Duration::from_secs(5), || {
        kernel.tasks(&session.id).unwrap()[0].status == TaskStatus::Failed
    })
    .await;
}

/// A sub-agent that started a background task does not return its summary
/// until the task finished and its loop consumed the completion note — and no
/// separate wake turn runs on the child.
#[tokio::test]
async fn sub_agent_waits_for_its_own_background_task() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["bgwork", "other"],
        vec![],
        noop_hooks(),
        Arc::new(|_, _, _| ToolOutput::ok("other-done")),
    ));
    let (kernel, _session, branch) = setup_split(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "mode": "fork", "background": false, "task": "wait for work" }),
            )]),
            LlmResponse::text("parent done"),
        ],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "bgwork",
                json!({ "ms": 100, "background": true }),
            )]),
            LlmResponse::text("early"),
            LlmResponse::text("final after task"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    let (done, seen) = wait_turn_collect(&mut rx, Some(&branch.0)).await;
    assert_eq!(done["method"], "turn.done");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let (text, err) = result_text(&nodes, "e1");
    assert!(!err, "{text}");
    assert!(text.starts_with("final after task"), "{text}");

    let child = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::Assistant { tool_calls, .. } => {
                tool_calls.iter().find(|c| c.name == "agent").map(|_| ())
            }
            _ => None,
        })
        .and_then(|_| {
            kernel
                .session_tree(&_session)
                .unwrap()
                .2
                .into_iter()
                .find(|b| matches!(b.kind, rumu_core::BranchKind::Agent { .. }))
        })
        .expect("agent child");
    let wake_starts = seen
        .iter()
        .filter(|v| {
            v["method"] == "turn.started"
                && v["params"]["branch"] == child.id.0
                && v["params"]["reason"].as_str() == Some("wake")
        })
        .count();
    assert_eq!(wake_starts, 0, "no separate wake turn on the child");

    let (cnodes, _) = kernel.branch_path(&child.id).unwrap();
    assert_eq!(
        task_note_count(&cnodes),
        1,
        "the child consumed its completion note"
    );
    assert!(cnodes.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { content, .. } if content.text == "final after task")));
    assert!(cnodes.iter().any(|n| matches!(&n.body,
        NodeBody::Assistant { content, .. } if content.text == "early")));
}

/// A kernel restart closes out agent branches that were still active: their
/// in-flight tasks are gone and they are never resumed.
#[tokio::test]
async fn kernel_start_closes_out_active_agent_branches() {
    let store = Arc::new(Store::in_memory());
    let kernel = Kernel::new(
        store.clone(),
        Arc::new(FakeLlm::new(vec![])),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let main = store.get_branch(&session.main_branch).unwrap().unwrap();
    let child = rumu_core::Branch {
        id: BranchId::new(),
        session: session.id.clone(),
        head: main.head.clone(),
        forked_from: Some(main.head.clone()),
        kind: rumu_core::BranchKind::Agent {
            task: "work".into(),
            parent_branch: main.id.clone(),
            call_id: "c1".into(),
            via_plugin: false,
            inherit: Default::default(),
        },
        status: BranchStatus::Active,
        created_at: rumu_core::now(),
    };
    store.insert_branch(&child).unwrap();

    let _restarted = Kernel::new(
        store.clone(),
        Arc::new(FakeLlm::new(vec![])),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let got = store.get_branch(&child.id).unwrap().unwrap();
    assert_eq!(got.status, BranchStatus::Done);
}

/// Effects returned by a background tool are posted under the plugin's own
/// source, before the kernel's completion note.
#[tokio::test]
async fn background_task_effects_keep_their_plugin_source() {
    let plugin = Arc::new(background_plugin(
        "bg",
        &["append_note"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _s, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "c1",
                "append_note",
                json!({ "background": true }),
            )]),
            LlmResponse::text("started"),
            LlmResponse::text("woken up"),
        ],
    )
    .await;
    kernel.set_wake_debounce(Duration::from_millis(10));
    let mut rx = kernel.tap_pushes();

    let (done, _, _) = turn(&kernel, &branch, &mut rx, "go").await;
    assert_eq!(done["method"], "turn.done");
    wait_push(
        &mut rx,
        |v| pushed_note_tag(v) == Some("task.done"),
        "task completion note",
    )
    .await;
    let (done2, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done2["method"], "turn.done");

    let (nodes, _) = kernel.branch_path(&branch).unwrap();
    let effect = nodes
        .iter()
        .position(|n| {
            matches!(&n.body,
            NodeBody::Note { source, content } if source == "bg" && content.text == "direct note")
        })
        .expect("the tool's effect, attributed to its plugin");
    let note = nodes
        .iter()
        .position(|n| matches!(&n.body,
            NodeBody::Note { source, content } if source == "system" && content.decode::<TaskDone>().is_some()))
        .expect("the completion note");
    assert!(effect < note, "effects land before the completion note");
}

/// The model sees a tool output capped to `OUTPUT_LIMIT`; a plugin invoke
/// gets the whole output (up to `INVOKE_OUTPUT_LIMIT`).
#[tokio::test]
async fn invoke_outputs_are_not_capped_to_the_model_limit() {
    let plugin = Arc::new(script_host(&["big"], vec![], noop_hooks(), noop_tools()));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new("c1", "big", json!({})),
                ToolCall::new("c2", "script", json!({ "calls": [{ "name": "big" }] })),
            ]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let (direct, _) = result_text(&nodes, "c1");
    assert!(direct.contains("[truncated"), "{} chars", direct.len());
    assert!(direct.chars().count() < rumu_core::OUTPUT_LIMIT + 100);
    let (inner, _) = result_text(&nodes, "c2");
    let rows: Value = serde_json::from_str(&inner).unwrap();
    assert_eq!(rows[0]["len"], 50_000);
    // The stored trace node keeps the model-sized output.
    let stored = nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::ToolResult {
                content,
                invoke: Some(_),
                ..
            } => Some(content.text.clone()),
            _ => None,
        })
        .expect("the invoke node");
    assert!(stored.contains("[truncated"));
}

/// `Cx::cancel_invokes` ends a plugin's still-running internal calls.
#[tokio::test]
async fn cancel_invokes_ends_running_internal_calls() {
    let plugin = Arc::new(script_host(
        &["orphan", "nap"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, _session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new("c1", "orphan", json!({}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let (_, nodes, _) = run_turn(&kernel, &branch, "go").await;
    let (text, _) = result_text(&nodes, "c1");
    assert!(text.starts_with("cancelled:"), "{text}");
}

// ---------- joining a running turn ----------

#[tokio::test]
async fn live_turns_snapshot_the_stream_in_flight() {
    let reply = "a reply long enough to stream in several chunks";
    let llm =
        ScriptedLlm::strict([LlmResponse::text(reply)]).with_chunk_delay(Duration::from_millis(20));
    let kernel = Kernel::new(
        Arc::new(Store::in_memory()),
        Arc::new(llm),
        vec![],
        Workspaces::default(),
    )
    .unwrap();
    let (session, _) = kernel.create_session("assistant".into()).await.unwrap();
    let branch = session.main_branch.clone();
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "hi").await.unwrap();

    // Mid-stream, the snapshot holds at least the deltas pushed so far.
    let mut seen = String::new();
    let mut last_seq = 0;
    while let Some(v) = rx.recv().await {
        if v["method"] != "llm.delta" {
            continue;
        }
        seen.push_str(v["params"]["text"].as_str().unwrap());
        last_seq = v["params"]["seq"].as_u64().unwrap();
        if last_seq == 2 {
            break;
        }
    }
    let live = kernel.live_turns(&session.id).unwrap();
    let streamed = &live[&branch];
    assert!(streamed.text.starts_with(&seen), "{streamed:?} vs {seen:?}");
    assert!(streamed.seq >= last_seq);

    // The loop's end clears it and announces the session's new activity.
    wait_turn(&mut rx, None).await;
    let announced = loop {
        let v = rx.recv().await.expect("session.updated at the loop's end");
        if v["method"] == "session.updated" {
            break v["params"]["session"].clone();
        }
    };
    assert!(announced["updated_at"].as_i64() > announced["seen_at"].as_i64());
    assert!(kernel.live_turns(&session.id).unwrap().is_empty());

    let info = kernel.mark_seen(&session.id).unwrap();
    assert!(info.seen_at >= Some(info.updated_at));
}

#[tokio::test]
async fn model_use_marker_is_written_only_when_the_pick_changes() {
    use rumu_core::{Effort, ModelUse};
    let (kernel, llm, _s, b) = setup(vec![], vec![LlmResponse::text("ok")]).await;
    let cheap = ModelUse {
        model: "rumu/cheap".into(),
        effort: Some(Effort::Low),
    };
    // The assistant's model is where a path starts.
    assert!(!kernel.use_model(&b, None).await.unwrap());
    assert!(kernel.use_model(&b, Some(cheap.clone())).await.unwrap());
    // The same pick again writes nothing.
    assert!(!kernel.use_model(&b, Some(cheap.clone())).await.unwrap());
    let (_, nodes, state) = run_turn(&kernel, &b, "hi").await;
    assert_eq!(state.model.as_ref(), Some(&cheap));
    let markers = nodes
        .iter()
        .filter(|n| {
            matches!(&n.body,
            NodeBody::Note { content, .. } if content.decode::<ModelUse>().is_some())
        })
        .count();
    assert_eq!(markers, 1);
    // The marker is state, never model-visible text.
    let seen = &llm.projections()[0];
    assert!(seen.iter().all(|n| !matches!(&n.body,
        NodeBody::Note { content, .. } if !content.text.is_empty())));

    let other = ModelUse {
        model: "p/m".into(),
        effort: None,
    };
    assert!(kernel.use_model(&b, Some(other.clone())).await.unwrap());
    let (_, st) = kernel.branch_path(&b).unwrap();
    assert_eq!(st.model, Some(other));
    // Back to the assistant's.
    assert!(kernel.use_model(&b, None).await.unwrap());
    let (_, st) = kernel.branch_path(&b).unwrap();
    assert_eq!(st.model, None);

    // A choice tells an absent model (leave it) from a null one (the
    // assistant's).
    let absent: rumu_kernel::Choice = serde_json::from_value(json!({})).unwrap();
    let null: rumu_kernel::Choice = serde_json::from_value(json!({ "model": null })).unwrap();
    assert_eq!((absent.model, null.model), (None, Some(None)));
}

/// A sub-agent with the whole context stays on its parent's model but may
/// change the effort; another model is refused, and only aliases are offered.
#[tokio::test]
async fn agent_full_context_changes_only_the_effort() {
    use rumu_core::{Effort, ModelUse};
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![
                ToolCall::new(
                    "e1",
                    "agent",
                    json!({ "mode": "fork", "background": false, "task": "quick look", "effort": "low" }),
                ),
                ToolCall::new(
                    "e2",
                    "agent",
                    json!({ "mode": "fork", "background": false, "task": "x", "model": "p/other" }),
                ),
            ]),
            LlmResponse::text("looked"),
            LlmResponse::text("parent done"),
        ],
    )
    .await;
    let parent = ModelUse {
        model: "p/m".into(),
        effort: Some(Effort::High),
    };
    kernel
        .use_model(&branch, Some(parent.clone()))
        .await
        .unwrap();

    let (_, nodes, state) = run_turn_on(&kernel, &branch, "go").await;
    assert_eq!(state.model, Some(parent));
    let child = kernel
        .session_tree(&session)
        .unwrap()
        .2
        .into_iter()
        .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { task, .. } if task == "quick look"))
        .expect("agent child");
    let (_, st) = kernel.branch_path(&child.id).unwrap();
    assert_eq!(
        st.model,
        Some(ModelUse {
            model: "p/m".into(),
            effort: Some(Effort::Low),
        })
    );
    let (text, err) = result_text(&nodes, "e2");
    assert!(err, "{text}");
    assert!(
        text.contains("`p/other` is not a model alias (aliases: none)"),
        "{text}"
    );
}

/// A path with no `model.use` marker runs on its assistant's model: an
/// effort-only sub-agent sets the effort on that one.
#[tokio::test]
async fn agent_effort_applies_to_the_assistants_model() {
    use rumu_core::{Effort, ModelUse};
    let plugin = Arc::new(plugin_with(
        "branch",
        &["agent"],
        vec![],
        noop_hooks(),
        noop_tools(),
    ));
    let (kernel, _llm, session, branch) = setup(
        vec![plugin],
        vec![
            LlmResponse::calls(vec![ToolCall::new(
                "e1",
                "agent",
                json!({ "mode": "fork", "background": false, "task": "quick look", "effort": "low" }),
            )]),
            LlmResponse::text("looked"),
            LlmResponse::text("parent done"),
        ],
    )
    .await;
    kernel.set_assistant_model(Arc::new(|_| {
        Some(ModelUse {
            model: "p/m".into(),
            effort: Some(Effort::High),
        })
    }));
    let (_, _, state) = run_turn_on(&kernel, &branch, "go").await;
    assert_eq!(state.model, None);
    let child = kernel
        .session_tree(&session)
        .unwrap()
        .2
        .into_iter()
        .find(|b| matches!(&b.kind, rumu_core::BranchKind::Agent { .. }))
        .expect("agent child");
    let (_, st) = kernel.branch_path(&child.id).unwrap();
    assert_eq!(
        st.model,
        Some(ModelUse {
            model: "p/m".into(),
            effort: Some(Effort::Low),
        })
    );
}

// ---------- side questions ----------

/// Ask a side question off `main`, wait for its turn, and return the side
/// branch with its path.
async fn ask_side(
    kernel: &Arc<Kernel>,
    main: &BranchId,
    quote: Option<rumu_core::Quote>,
    text: &str,
) -> (rumu_core::Branch, Vec<Node>) {
    let mut rx = kernel.tap_pushes();
    let side = kernel.side(main, quote, Content::text(text)).await.unwrap();
    wait_turn(&mut rx, Some(side.id.0.as_str())).await;
    let (nodes, _) = kernel.branch_path(&side.id).unwrap();
    (side, nodes)
}

#[tokio::test]
async fn side_question_forks_at_the_head_and_leaves_the_main_line_alone() {
    let (kernel, llm, _s, main) = setup(
        vec![],
        vec![
            LlmResponse::text("slow start doubles cwnd"),
            LlmResponse::text("it starts at one MSS"),
        ],
    )
    .await;
    let (_, before, _) = run_turn(&kernel, &main, "how does tcp start?").await;
    let reply = before.last().unwrap().id.clone();

    let quote = rumu_core::Quote {
        node: reply.clone(),
        text: "doubles cwnd".into(),
    };
    let (side, nodes) = ask_side(&kernel, &main, Some(quote.clone()), "why slow?").await;
    assert_eq!(side.forked_from.as_ref(), Some(&reply));
    assert!(matches!(&side.kind,
        rumu_core::BranchKind::Side { parent_branch, quote: Some(q) } if parent_branch == &main && q == &quote));
    assert_eq!(
        body_kinds(&nodes),
        vec![
            "marker[session.root]",
            "user",
            "assistant",
            "note",
            "user",
            "assistant"
        ]
    );

    // The model saw the main conversation, then the framing with the quote.
    let seen = llm.projections().pop().unwrap();
    assert!(seen.iter().any(|n| matches!(&n.body,
        NodeBody::Note { content, .. } if content.text.contains("> doubles cwnd"))));

    // The main line is untouched.
    let (after, _) = kernel.branch_path(&main).unwrap();
    assert_eq!(after.len(), before.len());
    let session = kernel.list_sessions().unwrap().remove(0);
    assert_eq!(session.main_branch, main);
    assert_eq!(session.title, "how does tcp start?");

    // It hangs off the main line, it is not one.
    assert!(matches!(
        kernel.move_head(&side.id, &side.head, false).await,
        Err(rumu_kernel::Error::NotMainLine(_))
    ));
    assert!(matches!(
        kernel.side(&side.id, None, Content::text("again")).await,
        Err(rumu_kernel::Error::NotMainLine(_))
    ));
}

#[tokio::test]
async fn side_question_refuses_tool_calls() {
    let plugin = Arc::new(plugin_with(
        "calc",
        &["plus"],
        vec![],
        noop_hooks(),
        plus_tool(),
    ));
    let (kernel, _llm, _s, main) = setup(
        vec![plugin],
        vec![
            LlmResponse::text("hi"),
            LlmResponse::calls(vec![ToolCall::new("c1", "plus", json!({"a": 1, "b": 1}))]),
            LlmResponse::text("two, from memory"),
            LlmResponse::calls(vec![ToolCall::new("c2", "plus", json!({"a": 2, "b": 2}))]),
            LlmResponse::calls(vec![ToolCall::new("c3", "plus", json!({"a": 3, "b": 3}))]),
        ],
    )
    .await;
    run_turn(&kernel, &main, "hello").await;

    let (_, nodes) = ask_side(&kernel, &main, None, "what is 1+1?").await;
    let (text, err) = result_text(&nodes, "c1");
    assert!(err && text.contains("no tools"), "{text}");
    assert_eq!(body_kinds(&nodes).last().unwrap(), "assistant");

    // A second batch after the refusal ends the turn instead of looping.
    let (_, nodes) = ask_side(&kernel, &main, None, "and 2+2?").await;
    assert!(result_text(&nodes, "c2").1 && result_text(&nodes, "c3").1);
    assert_eq!(body_kinds(&nodes).last().unwrap(), "tool_result");
}

// ---------- decisions with a timeout ----------

/// Puts a question to the user with a one-minute timeout and reports how it
/// ended.
struct Asker;

#[async_trait]
impl Plugin for Asker {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "asker".into(),
            instructions: None,
            tools: vec![tool_spec("ask_now")],
            hooks: vec![],
        }
    }

    async fn call_tool(&self, _name: &str, _args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        let answer = cx
            .decide(json!({ "kind": "test" }), Some(Duration::from_secs(60)))
            .await?;
        Ok(ToolOutput::ok(match answer {
            Some(a) => format!("answered {a}"),
            None => "timed out".into(),
        }))
    }
}

async fn asking() -> (Arc<Kernel>, BranchId, PushRx, String) {
    let (kernel, _, _, branch) = setup(
        vec![Arc::new(Asker)],
        vec![
            LlmResponse::calls([ToolCall::new("c1", "ask_now", json!({}))]),
            LlmResponse::text("done"),
        ],
    )
    .await;
    let mut rx = kernel.tap_pushes();
    kernel.prompt(&branch, "go").await.unwrap();
    let id = loop {
        let v = rx.recv().await.unwrap();
        if v["method"] == "decision.requested" {
            assert!(v["params"]["expires_at"].as_i64().is_some(), "{v}");
            break v["params"]["id"].as_str().unwrap().to_owned();
        }
    };
    (kernel, branch, rx, id)
}

fn tool_result(kernel: &Kernel, branch: &BranchId) -> String {
    let (nodes, _) = kernel.branch_path(branch).unwrap();
    nodes
        .iter()
        .find_map(|n| match &n.body {
            NodeBody::ToolResult { content, .. } => Some(content.text.clone()),
            _ => None,
        })
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_decision_runs_out() {
    let (kernel, branch, mut rx, _) = asking().await;
    let status = || kernel.list_sessions().unwrap()[0].status;
    assert_eq!(status(), rumu_core::SessionStatus::Waiting);
    let (done, seen) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");
    assert!(seen.iter().any(|v| v["method"] == "decision.resolved"));
    assert!(kernel.decisions().is_empty());
    assert_ne!(status(), rumu_core::SessionStatus::Waiting);
    assert_eq!(tool_result(&kernel, &branch), "timed out");
}

#[tokio::test(start_paused = true)]
async fn a_held_decision_waits_for_the_answer() {
    let (kernel, branch, mut rx, id) = asking().await;
    kernel.hold_decision(&id).unwrap();
    let held = loop {
        let v = rx.recv().await.unwrap();
        if v["method"] != "session.updated" {
            break v;
        }
    };
    assert_eq!(held["method"], "decision.held", "{held}");
    assert!(kernel.decisions()[0].get("expires_at").is_none());
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(kernel.decisions().len(), 1, "no clock once held");
    kernel.answer_decision(&id, json!("yes")).unwrap();
    let (done, _) = collect_turn(&mut rx, &branch.0).await;
    assert_eq!(done["method"], "turn.done", "{done}");
    assert_eq!(tool_result(&kernel, &branch), "answered \"yes\"");
}
