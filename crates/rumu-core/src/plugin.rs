use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::{
    Attachment, Branch, BranchId, Content, Error, Event, Index, Node, PathState, Result, SessionId,
    SessionInfo, Tagged, TaskId, TaskInfo,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tools that a `code` script may not invoke through `rumu.*` / `rumu.call`
/// because `code` itself would nest. The kernel rejects it before
/// `tool.pre` dispatch, and the `code` plugin leaves it out of its API stub.
///
/// `agent` is allowed: a script may fan out agents with `Promise.all`;
/// they fork at the enclosing `code` call.
pub const CODE_DENIED_TOOLS: &[&str] = &["code"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    /// Static text appended to the (frozen) system prompt.
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub hooks: Vec<HookSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object.
    pub schema: Value,
    /// Whether this tool may run as a kernel background task. For such tools
    /// the kernel adds an optional `background: boolean` property to the
    /// schema it sends to the model and strips it before calling the plugin.
    #[serde(default)]
    pub background: bool,
    /// JSON Schema of the structured result a `code` script receives instead
    /// of the text. A tool that sets it attaches that value as its output's
    /// [`Content::data`] when invoked by a plugin (see [`Call::parent`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Safe to run concurrently with other calls of the same `code` script
    /// (read-only tools, `agent`). Other calls run one at a time, in order.
    #[serde(default)]
    pub parallel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookSpec {
    /// Unique within the plugin; passed back to `on_hook`.
    pub id: String,
    /// Predicate tree evaluated by the kernel.
    /// Should normally contain an `event:` key.
    #[serde(rename = "match")]
    pub matcher: Value,
    /// Lower runs first.
    #[serde(default)]
    pub priority: i32,
}

/// A plugin-authored tail append: a note's content. Non-empty text projects
/// as `<note source="..">`; empty text is a marker (payload only). The kernel
/// fills `NodeBody::Note::source` with the owning plugin's name.
pub type Effect = Content;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: Content,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            content: Content::text(text),
            ..Default::default()
        }
    }

    pub fn err(text: impl std::fmt::Display) -> Self {
        Self {
            content: Content::text(text.to_string()),
            is_error: true,
            ..Default::default()
        }
    }

    /// Attach a structured payload to the output text.
    pub fn with_data<T: crate::Structured>(mut self, data: &T) -> Self {
        self.content.data = Some(Tagged::new(data));
        self
    }

    /// Attach images to the output.
    pub fn with_attachments(mut self, attachments: impl IntoIterator<Item = Attachment>) -> Self {
        self.content.attachments.extend(attachments);
        self
    }

    pub fn with_effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }

    pub fn with_effects(mut self, effects: impl IntoIterator<Item = Effect>) -> Self {
        self.effects.extend(effects);
        self
    }

    /// The model-facing text, capped to [`crate::OUTPUT_LIMIT`]. Only the
    /// text is touched: a cut `[Image #N]` anchor simply leaves its image
    /// unanchored, and [`Content::segments`] places unanchored images after
    /// the text — attachments always survive the cap.
    ///
    /// The kernel applies this where an output reaches the model (tool
    /// results and task completions); a plugin invoke gets
    /// [`ToolOutput::cap_invoke`] instead, so tool bodies never call it.
    pub fn cap(mut self) -> Self {
        self.content.text = crate::truncate_output(std::mem::take(&mut self.content.text));
        self
    }

    /// The plugin-facing cap: [`crate::INVOKE_OUTPUT_LIMIT`], cut in the middle.
    pub fn cap_invoke(mut self) -> Self {
        self.content.text = crate::truncate_middle(
            std::mem::take(&mut self.content.text),
            crate::INVOKE_OUTPUT_LIMIT,
        );
        self
    }
}

impl From<&str> for ToolOutput {
    fn from(text: &str) -> Self {
        Self::ok(text)
    }
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self::ok(text)
    }
}

impl From<Content> for ToolOutput {
    fn from(content: Content) -> Self {
        Self {
            content,
            ..Default::default()
        }
    }
}

/// A tool body's result.
///
/// `Ok` is any success value convertible into a [`ToolOutput`]; `Err` is the
/// boundary [`ToolError`]. Helpers return `ToolResult<T>` so failures
/// propagate with `?`, and [`ToolResultExt::finish`] closes the body at the
/// `Plugin` boundary (capping the model-facing text).
pub type ToolResult<T = ToolOutput> = std::result::Result<T, ToolError>;

/// Why a tool call failed.
#[derive(Debug)]
pub enum ToolError {
    /// Show the model an error result; the call still completes. Boxed so a
    /// [`ToolResult`] stays small.
    Model(Box<ToolOutput>),
    /// Abort the call; the kernel reports it like any other core error.
    Fatal(Error),
}

impl ToolError {
    /// A model-facing failure message.
    pub fn model(message: impl std::fmt::Display) -> Self {
        Self::Model(Box::new(ToolOutput::err(message.to_string())))
    }

    /// Attach one effect to a model-facing failure (a fatal error is
    /// returned unchanged).
    pub fn with_effect(self, effect: Effect) -> Self {
        match self {
            Self::Model(output) => Self::Model(Box::new(output.with_effect(effect))),
            fatal => fatal,
        }
    }

    /// Attach effects to a model-facing failure (a fatal error is returned
    /// unchanged).
    pub fn with_effects(self, effects: impl IntoIterator<Item = Effect>) -> Self {
        match self {
            Self::Model(output) => Self::Model(Box::new(output.with_effects(effects))),
            fatal => fatal,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Model(output) => f.write_str(&output.content.text),
            Self::Fatal(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Model(_) => None,
            Self::Fatal(error) => Some(error),
        }
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self::model(message)
    }
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::model(message)
    }
}

impl From<ToolOutput> for ToolError {
    fn from(output: ToolOutput) -> Self {
        Self::Model(Box::new(output))
    }
}

impl From<Error> for ToolError {
    fn from(error: Error) -> Self {
        Self::Fatal(error)
    }
}

/// Close a [`ToolResult`] at the `Plugin::call_tool` boundary.
pub trait ToolResultExt {
    /// Success values become the output; model-facing failures become
    /// `is_error` outputs; fatal failures surface as plugin errors.
    fn finish(self) -> Result<ToolOutput>;
}

impl<T: Into<ToolOutput>> ToolResultExt for ToolResult<T> {
    fn finish(self) -> Result<ToolOutput> {
        match self {
            Ok(output) => Ok(output.into()),
            Err(ToolError::Model(mut output)) => {
                output.is_error = true;
                Ok(*output)
            }
            Err(ToolError::Fatal(error)) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_converts_success_values() {
        let out = ToolResult::<&str>::Ok("done").finish().unwrap();
        assert_eq!(out.content.text, "done");
        assert!(!out.is_error);
    }

    #[test]
    fn finish_marks_model_failures_as_errors() {
        let out = ToolResult::<ToolOutput>::Err("nope".into())
            .finish()
            .unwrap();
        assert_eq!(out.content.text, "nope");
        assert!(out.is_error);
    }

    #[test]
    fn finish_keeps_model_failure_effects() {
        let effect = Content::text("note");
        let out = ToolResult::<ToolOutput>::Err(
            ToolOutput::err("nope").with_effect(effect.clone()).into(),
        )
        .finish()
        .unwrap();
        assert!(out.is_error);
        assert_eq!(out.effects, vec![effect]);
    }

    #[test]
    fn finish_propagates_fatal_errors() {
        let err = ToolResult::<ToolOutput>::Err(Error::msg("boom").into())
            .finish()
            .unwrap_err();
        assert_eq!(err.to_string(), "boom");
    }

    #[test]
    fn cap_bounds_the_model_facing_text() {
        let long = "x".repeat(crate::OUTPUT_LIMIT + 10);
        let out = ToolOutput::ok(long).cap();
        assert!(out.content.text.contains("[truncated"));
        assert!(out.content.text.chars().count() < crate::OUTPUT_LIMIT + 100);
    }

    #[test]
    fn cap_keeps_attachments_even_when_the_cut_takes_their_anchor() {
        let image = Attachment {
            kind: crate::AttachmentKind::Image,
            hash: "0".repeat(64),
            mime: "image/png".into(),
            name: Some("a.png".into()),
            width: 4,
            height: 3,
            size: 10,
        };
        let long = format!("{} [Image #1]", "x".repeat(crate::OUTPUT_LIMIT + 10));
        let out = ToolOutput::ok(long).with_attachments([image]).cap();
        assert!(out.content.text.contains("[truncated"));
        assert_eq!(out.content.attachments.len(), 1);
        // The anchor is gone, so the image follows the text unanchored.
        let rendered = out.content.rendered();
        assert!(!rendered.contains("[Image #1]"), "{rendered}");
        assert!(rendered.ends_with("\n[image: a.png 4×3]"), "{rendered}");
    }
}

/// What a hook returns: effects to apply and a verdict on the event.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookOutput {
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub decision: Decision,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum Decision {
    #[default]
    Pass,
    /// Only honoured on interceptable events. Short-circuits.
    Block { reason: String },
    /// JSON merge patch (RFC 7386) applied to `Event::payload`. Interceptable events only.
    Rewrite { patch: Value },
}

/// A tool-call progress update. Text appends are cumulative in order; data is
/// replaced wholesale (the latest wins).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Progress {
    Append(String),
    Data(Tagged),
}

/// Read-only session history served by [`Cx::history`].
pub trait History: Send + Sync {
    /// Every session, most recently updated first.
    fn sessions(&self) -> Result<Vec<SessionInfo>>;
    /// One session's info plus all of its nodes and branches.
    fn session_tree(&self, session: &SessionId) -> Result<(SessionInfo, Vec<Node>, Vec<Branch>)>;
    /// Every kernel task owned by a branch of `session`, oldest first,
    /// including the ones earlier kernels left behind (a restart marks a task
    /// that was running `lost`).
    fn tasks(&self, session: &SessionId) -> Result<Vec<TaskInfo>>;
    /// One kernel task plus its full buffered output (already annotated when
    /// the buffer dropped earlier text). `None` when no such task exists.
    fn task(&self, id: &TaskId) -> Result<Option<(TaskInfo, String)>>;
}

/// One out-of-band post to a branch: effects to append, and whether the post
/// may start a turn when the branch is idle. `paced` marks a poster that
/// bounds its own rate and lifetime (a `bash` watch): a wake made only of
/// paced posts does not count toward the kernel's wake-storm guard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Post {
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub wake: bool,
    #[serde(default)]
    pub paced: bool,
}

/// The kernel's out-of-band delivery channel, one per plugin.
///
/// The source of a post is the attaching plugin's name. Delivery is async:
/// [`Inbox::post`] validates (branch exists, content invariant) and enqueues.
pub trait Inbox: Send + Sync {
    /// Validate and enqueue a post for `branch`. Returns an error when the
    /// branch does not exist or a note is neither text nor data.
    fn post(&self, branch: &BranchId, post: Post) -> Result<()>;
}

/// Kernel services behind [`Cx`].
///
/// Implemented by the kernel (and by test fakes); plugins should use [`Cx`]
/// rather than implementing this trait.
#[doc(hidden)]
#[async_trait]
pub trait Runtime: History {
    /// What the model sees of the branch ([`crate::view`] of the nodes from
    /// root to head), followed by appends still queued for the next safe
    /// point (so a marker recorded earlier in a tool batch is visible to the
    /// batch's later calls). Nodes a compaction summarized are not in it.
    fn path(&self, branch: &BranchId) -> Result<Arc<[Node]>>;
    /// The path state derived from the same view as [`Runtime::path`].
    fn state(&self, branch: &BranchId) -> Result<Arc<PathState>>;
    /// Run a tool through `tool.pre` → tool → `tool.post` inside the branch's
    /// running loop. `caller` is the invoking plugin's manifest name,
    /// `parent_call` the enclosing model call id, and `depth` the nesting
    /// depth of this invoke (direct model calls are depth 0).
    async fn invoke(
        &self,
        branch: &BranchId,
        caller: &str,
        parent_call: &str,
        depth: usize,
        name: &str,
        args: Value,
    ) -> Result<InnerCall>;
    /// Cancel every internal call of `parent_call` that is still running;
    /// later invokes under it are cancelled at once.
    fn cancel_invokes(&self, _branch: &BranchId, _parent_call: &str) {}
    /// Put `request` to the user on behalf of the tool call `call_id` and
    /// wait for the answer (see [`Cx::decide`]).
    async fn decide(
        &self,
        _branch: &BranchId,
        _call_id: &str,
        _request: Value,
        _timeout: Option<Duration>,
    ) -> Result<Option<Value>> {
        Err(Error::msg("this host cannot ask the user"))
    }
    /// Start `new` as a session of its own (see [`Cx::start_session`]).
    async fn start_session(
        &self,
        _branch: &BranchId,
        _new: NewSession,
    ) -> Result<(SessionId, BranchId)> {
        Err(Error::msg("this host cannot start sessions"))
    }
    /// Wait until `target` is idle (see [`Cx::wait_idle`]).
    async fn wait_idle(&self, _target: &BranchId) -> Result<()> {
        Err(Error::msg("this host cannot wait on branches"))
    }
    /// Message the agent on `target` (see [`Cx::send`]).
    async fn send(&self, _target: &BranchId, _content: Content) -> Result<()> {
        Err(Error::msg("this host cannot message branches"))
    }
    /// Cancel the task `id` if `branch` owns it (see [`Cx::kill_task`]).
    async fn kill_task(&self, _branch: &BranchId, _id: &TaskId) -> Result<TaskInfo> {
        Err(Error::msg("this host has no tasks"))
    }
}

/// A session a tool call starts (`Cx::start_session`): who runs it, what it
/// mounts, the model asked for, and its first prompt.
#[derive(Debug, Clone)]
pub struct NewSession {
    /// The assistant deck the session runs as.
    pub assistant: String,
    /// Its mounts: the deck's defaults and the project.
    pub mounts: Vec<crate::CardMount>,
    /// The model alias asked for, and the effort.
    pub model: Option<String>,
    pub effort: Option<crate::Effort>,
    pub prompt: Content,
}

/// Where a tool call's progress goes, chosen by the kernel when it builds the
/// call's [`Cx`]: pushed to clients for a foreground call, into the task's
/// buffer for a background task.
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// The tool call a [`Cx`] belongs to. Hooks run without one.
#[derive(Clone)]
pub struct Call {
    pub id: String,
    /// The enclosing model call id when this call was issued from a plugin
    /// (`caller` = that plugin's name); `None` for direct model calls.
    pub parent: Option<String>,
    /// True when this call runs as a kernel background task; tools that
    /// normally enforce a foreground timeout may skip it.
    pub background: bool,
    progress: ProgressSink,
}

impl Call {
    #[doc(hidden)]
    pub fn new(
        id: String,
        parent: Option<String>,
        background: bool,
        progress: ProgressSink,
    ) -> Self {
        Self {
            id,
            parent,
            background,
            progress,
        }
    }
}

/// Per-call context handed to plugins.
#[derive(Clone)]
pub struct Cx {
    pub session: SessionId,
    pub branch: BranchId,
    /// Set for `call_tool`; `None` in hooks.
    call: Option<Call>,
    /// The manifest name of the plugin this context was built for. Private
    /// (like `depth`): `invoke` reports it as `caller`, so a plugin must not
    /// be able to rewrite it on a clone.
    plugin: Option<String>,
    /// Invoke nesting depth (0 for a direct model tool call).
    depth: usize,
    runtime: Arc<dyn Runtime>,
    index: Arc<OnceLock<Arc<Index>>>,
}

impl Cx {
    #[doc(hidden)]
    pub fn new(
        session: SessionId,
        branch: BranchId,
        call: Option<Call>,
        plugin: Option<String>,
        depth: usize,
        runtime: Arc<dyn Runtime>,
    ) -> Self {
        Self {
            session,
            branch,
            call,
            plugin,
            depth,
            runtime,
            index: Arc::new(OnceLock::new()),
        }
    }

    /// The tool call this context belongs to (`None` in hooks).
    pub fn call(&self) -> Option<&Call> {
        self.call.as_ref()
    }

    /// The manifest name of the plugin this context was built for.
    pub fn plugin(&self) -> Option<&str> {
        self.plugin.as_deref()
    }

    /// Invoke nesting depth (0 for a direct model tool call).
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// The model's view of the path plus queued appends (see
    /// [`Runtime::path`]).
    pub fn path(&self) -> Result<Arc<[Node]>> {
        self.runtime.path(&self.branch)
    }

    /// The state derived from the whole root→head path (plus queued
    /// appends), compacted nodes included.
    pub fn state(&self) -> Result<Arc<PathState>> {
        self.runtime.state(&self.branch)
    }

    /// The structured payloads on [`Cx::path`], built once per context.
    pub fn index(&self) -> Result<Arc<Index>> {
        if let Some(index) = self.index.get() {
            return Ok(index.clone());
        }
        let built = Arc::new(Index::build(&self.path()?));
        let _ = self.index.set(built.clone());
        // Another clone may have won the race; either index is equivalent.
        Ok(self.index.get().cloned().unwrap_or(built))
    }

    /// Read-only session history.
    pub fn history(&self) -> &dyn History {
        let history: &dyn History = self.runtime.as_ref();
        history
    }

    /// Run a tool inside the running loop (`tool.pre` → tool → `tool.post`).
    /// Only available within a foreground tool call; errors otherwise (and
    /// inside a background task).
    pub async fn invoke(&self, tool: &str, args: Value) -> Result<InnerCall> {
        let Some(call) = &self.call else {
            return Err(Error::NoCallContext);
        };
        if call.background {
            return Err(Error::InvokeInBackground);
        }
        let parent = call.parent.as_deref().unwrap_or(&call.id);
        let caller = self.plugin.as_deref().unwrap_or("unknown");
        self.runtime
            .invoke(&self.branch, caller, parent, self.depth + 1, tool, args)
            .await
    }

    /// Cancel this call's still-running [`Cx::invoke`]s (a script that ended
    /// without awaiting them). Sub-agents unwind as aborted.
    pub fn cancel_invokes(&self) {
        if let Some(call) = &self.call {
            let parent = call.parent.as_deref().unwrap_or(&call.id);
            self.runtime.cancel_invokes(&self.branch, parent);
        }
    }

    /// Put `request` to the user and wait for the answer: a `decision.requested`
    /// push the client answers with `decision.answer`. Aborting the turn
    /// cancels the wait (an error). With a `timeout`, `None` when it runs out
    /// before the user starts on it (`decision.hold`). Only available within
    /// a tool call.
    pub async fn decide(&self, request: Value, timeout: Option<Duration>) -> Result<Option<Value>> {
        let Some(call) = &self.call else {
            return Err(Error::NoCallContext);
        };
        self.runtime
            .decide(&self.branch, &call.id, request, timeout)
            .await
    }

    /// Start a session of its own and prompt it; it runs on by itself, like
    /// one the user started. Returns its id and main branch.
    pub async fn start_session(&self, new: NewSession) -> Result<(SessionId, BranchId)> {
        self.runtime.start_session(&self.branch, new).await
    }

    /// Wait until the branch `target` (of any session) is idle: no turn
    /// running or about to, no sub-agent still at work.
    pub async fn wait_idle(&self, target: &BranchId) -> Result<()> {
        self.runtime.wait_idle(target).await
    }

    /// Message the agent on the branch `target` (of any session): into its
    /// running turn, or starting one. It counts toward that branch's wake
    /// storm guard. Who may message whom is the caller's to check.
    pub async fn send(&self, target: &BranchId, content: Content) -> Result<()> {
        self.runtime.send(target, content).await
    }

    /// Cancel the background task `id` and return its final status. Only a
    /// task of this branch or of one of its sub-agents may be cancelled; one
    /// that already ended is reported as it ended.
    pub async fn kill_task(&self, id: &TaskId) -> Result<TaskInfo> {
        self.runtime.kill_task(&self.branch, id).await
    }

    /// Push a progress update for this tool call. A no-op outside a tool call.
    pub fn progress(&self, update: Progress) {
        if let Some(call) = &self.call {
            (call.progress)(update);
        }
    }
}

/// The result of a tool call issued from inside a `code` script.
#[derive(Debug, Clone)]
pub struct InnerCall {
    pub output: ToolOutput,
    /// The hook's block reason, when `tool.pre` blocked the call. The script
    /// sees a `BlockedError` with this message.
    pub blocked: Option<String>,
    pub dur_ms: u64,
}

#[async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> Manifest;

    /// Receive the kernel's out-of-band delivery channel, bound to this
    /// plugin's manifest name. Called once, after construction.
    fn attach(&self, _inbox: Arc<dyn Inbox>) {}

    async fn on_hook(&self, hook_id: &str, event: &Event, cx: &Cx) -> Result<HookOutput> {
        let _ = (hook_id, event, cx);
        Ok(HookOutput::default())
    }

    /// Handle a tool call. The default is for hook-only plugins; a plugin
    /// that lists tools in its manifest overrides it.
    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> Result<ToolOutput> {
        let _ = (args, cx);
        Ok(ToolOutput::err(format!(
            "plugin `{}` has no tool `{name}`",
            self.manifest().name
        )))
    }
}
