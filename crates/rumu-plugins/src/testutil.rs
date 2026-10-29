//! Shared test helpers (unit tests only): an in-memory `Runtime` with a
//! configurable path/state/history and scripted internal-call answers.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rumu_core::{
    Branch, BranchId, Call, Content, Cx, Error, History, Inbox, InnerCall, Node, NodeBody,
    PathState, Post, Progress, ProgressSink, Result, Runtime, SessionId, SessionInfo, TaskId,
    TaskInfo, ToolOutput,
};
use serde_json::Value;

/// One scripted answer for `Runtime::invoke` (the `code` plugin's bridge).
#[derive(Clone, Default)]
pub(crate) struct ScriptedInner {
    pub text: String,
    pub is_error: bool,
    pub blocked: Option<String>,
    pub dur_ms: u64,
}

impl ScriptedInner {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
            ..Default::default()
        }
    }

    pub fn blocked(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            text: format!("Blocked by hook: {reason}"),
            is_error: true,
            blocked: Some(reason),
            ..Default::default()
        }
    }
}

/// One row returned by `session_tree`: `(session id, info, nodes, branches)`.
pub type SessionTreeRow = (SessionId, SessionInfo, Vec<Node>, Vec<Branch>);

#[derive(Default)]
pub(crate) struct FakeRuntime {
    pub state: Mutex<PathState>,
    /// Nodes returned by `path` (tests stage a branch path here).
    pub nodes: Mutex<Vec<Node>>,
    /// Answers handed out in order by `invoke`.
    pub inner_specs: Mutex<VecDeque<ScriptedInner>>,
    /// Every internal call received: (branch, parent_call, name, args).
    pub inner_calls: Mutex<Vec<(BranchId, String, String, Value)>>,
    /// Sessions returned by `sessions` (already in display order).
    pub sessions: Mutex<Vec<SessionInfo>>,
    /// Session trees returned by `session_tree`, keyed by session id.
    pub session_trees: Mutex<Vec<SessionTreeRow>>,
    /// Every progress update received: (branch, call_id, update).
    pub progress: Mutex<Vec<(BranchId, String, Progress)>>,
    /// Kernel tasks returned by `tasks` / `task`: (info, buffered output).
    pub tasks: Mutex<Vec<(TaskInfo, String)>>,
}

impl FakeRuntime {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A `Cx` on a fresh branch with this runtime and call id `call-1`; the
    /// staged nodes back [`Cx::index`] as well as [`Cx::path`].
    pub fn cx(self: &Arc<Self>) -> Cx {
        self.cx_with(false)
    }

    /// Like [`FakeRuntime::cx`], optionally as a background task's call.
    /// Progress updates are recorded in `progress`.
    pub fn cx_with(self: &Arc<Self>, background: bool) -> Cx {
        let runtime: Arc<dyn Runtime> = self.clone();
        let branch = BranchId::new();
        let sink: ProgressSink = {
            let fake = self.clone();
            let branch = branch.clone();
            Arc::new(move |update| {
                fake.progress
                    .lock()
                    .unwrap()
                    .push((branch.clone(), "call-1".into(), update));
            })
        };
        Cx::new(
            SessionId::new(),
            branch,
            Some(Call::new("call-1".into(), None, background, sink)),
            Some("test".into()),
            0,
            runtime,
        )
    }

    /// Mirror the kernel's safe point for one tool output: append its ok
    /// ToolResult and effects to the staged path, so later calls see them.
    pub fn apply_output(&self, call_id: &str, out: &ToolOutput) {
        let mut nodes = self.nodes.lock().unwrap();
        nodes.push(Node {
            id: rumu_core::NodeId::new(),
            parent: None,
            session: Default::default(),
            at: 0,
            dur_ms: None,
            body: NodeBody::ToolResult {
                call_id: call_id.into(),
                content: rumu_core::Content::text("ok"),
                is_error: false,
            },
        });
        for effect in &out.effects {
            nodes.push(Node {
                id: rumu_core::NodeId::new(),
                parent: None,
                session: Default::default(),
                at: 0,
                dur_ms: None,
                body: NodeBody::Note {
                    source: "workspace".into(),
                    content: effect.clone(),
                },
            });
        }
    }
}

impl History for FakeRuntime {
    fn sessions(&self) -> Result<Vec<SessionInfo>> {
        Ok(self.sessions.lock().unwrap().clone())
    }

    fn session_tree(&self, session: &SessionId) -> Result<(SessionInfo, Vec<Node>, Vec<Branch>)> {
        let trees = self.session_trees.lock().unwrap();
        let (info, nodes, branches) = trees
            .iter()
            .find(|(id, ..)| id == session)
            .map(|(_, i, n, b)| (i.clone(), n.clone(), b.clone()))
            .ok_or_else(|| Error::msg(format!("no such session {session}")))?;
        Ok((info, nodes, branches))
    }

    fn tasks(&self, session: &SessionId) -> Result<Vec<TaskInfo>> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| &t.session == session)
            .map(|(t, _)| t.clone())
            .collect())
    }

    fn task(&self, id: &TaskId) -> Result<Option<(TaskInfo, String)>> {
        Ok(self
            .tasks
            .lock()
            .unwrap()
            .iter()
            .find(|(t, _)| &t.id == id)
            .cloned())
    }
}

#[async_trait]
impl Runtime for FakeRuntime {
    fn path(&self, _branch: &BranchId) -> Result<Arc<[Node]>> {
        Ok(self.nodes.lock().unwrap().clone().into())
    }

    fn state(&self, _branch: &BranchId) -> Result<Arc<PathState>> {
        Ok(Arc::new(self.state.lock().unwrap().clone()))
    }

    async fn invoke(
        &self,
        branch: &BranchId,
        _caller: &str,
        parent_call: &str,
        _depth: usize,
        name: &str,
        args: Value,
    ) -> Result<InnerCall> {
        self.inner_calls.lock().unwrap().push((
            branch.clone(),
            parent_call.to_owned(),
            name.to_owned(),
            args,
        ));
        let spec = self
            .inner_specs
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default();
        Ok(InnerCall {
            output: ToolOutput {
                content: rumu_core::Content::text(spec.text),
                is_error: spec.is_error,
                ..Default::default()
            },
            blocked: spec.blocked,
            dur_ms: spec.dur_ms,
        })
    }
}

/// The sample RUMU_HOME used by the tests.
pub(crate) fn fixture_home() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/home")
}

/// An [`Inbox`] that records posts instead of delivering them.
#[derive(Default)]
pub(crate) struct FakeInbox {
    pub posts: Mutex<Vec<(BranchId, Post)>>,
}

impl FakeInbox {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn posts(&self) -> Vec<(BranchId, Post)> {
        self.posts.lock().unwrap().clone()
    }
}

impl Inbox for FakeInbox {
    fn post(&self, branch: &BranchId, post: Post) -> Result<()> {
        for effect in &post.effects {
            effect.validate_note()?;
        }
        self.posts.lock().unwrap().push((branch.clone(), post));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_inbox_records_valid_posts_and_rejects_empty_notes() {
        let inbox = FakeInbox::arc();
        let branch = BranchId::new();
        inbox
            .post(
                &branch,
                Post {
                    effects: vec![Content::text("hi")],
                    wake: true,
                },
            )
            .unwrap();
        assert_eq!(inbox.posts().len(), 1);
        assert!(inbox.posts()[0].1.wake);
        assert!(
            inbox
                .post(
                    &branch,
                    Post {
                        effects: vec![Content::default()],
                        wake: false,
                    },
                )
                .is_err()
        );
    }
}
