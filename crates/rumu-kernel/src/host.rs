//! `impl Runtime` — kernel services exposed to plugins through [`Cx`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rumu_core::{
    Branch, BranchId, Delegation, DelegationReply, History, InnerCall, Node, PathState, Progress,
    ProgressSink, Result, Runtime, SessionId, SessionInfo, TaskId, TaskInfo,
};
use serde_json::{Value, json};

use crate::Kernel;

pub(crate) struct KernelRuntime(pub Arc<Kernel>);

impl History for KernelRuntime {
    fn sessions(&self) -> Result<Vec<SessionInfo>> {
        Ok(self.0.list_sessions()?)
    }

    fn session_tree(&self, session: &SessionId) -> Result<(SessionInfo, Vec<Node>, Vec<Branch>)> {
        Ok(self.0.session_tree(session)?)
    }

    fn tasks(&self, session: &SessionId) -> Result<Vec<TaskInfo>> {
        Ok(self.0.tasks(session)?)
    }

    fn task(&self, id: &TaskId) -> Result<Option<(TaskInfo, String)>> {
        Ok(self.0.task(id)?)
    }
}

#[async_trait]
impl Runtime for KernelRuntime {
    /// Includes appends queued for the next safe point (see
    /// `Kernel::pending_path_nodes`).
    fn path(&self, branch: &BranchId) -> Result<Arc<[Node]>> {
        Ok(rumu_core::view(&self.0.pending_path_nodes(branch)?).into())
    }

    fn state(&self, branch: &BranchId) -> Result<Arc<PathState>> {
        Ok(Arc::new(
            self.0.state_of_nodes(&self.0.pending_path_nodes(branch)?),
        ))
    }

    /// A plugin-originated internal call: the same `tool.pre` → tool →
    /// `tool.post` path as model calls, inside the current batch.
    async fn invoke(
        &self,
        branch: &BranchId,
        caller: &str,
        parent_call: &str,
        depth: usize,
        name: &str,
        args: Value,
    ) -> Result<InnerCall> {
        self.0
            .exec_invoke(branch, caller, parent_call, depth, name, args)
            .await
            .map_err(Into::into)
    }

    async fn decide(
        &self,
        branch: &BranchId,
        call_id: &str,
        request: Value,
        timeout: Option<Duration>,
    ) -> Result<Option<Value>> {
        Ok(self.0.decide(branch, call_id, request, timeout).await?)
    }

    async fn delegate(&self, branch: &BranchId, delegation: Delegation) -> Result<DelegationReply> {
        Ok(self.0.delegate(branch, delegation).await?)
    }

    fn cancel_invokes(&self, _branch: &BranchId, parent_call: &str) {
        let mut inner = self.0.inner.lock().unwrap();
        if let Some(token) = inner.inner_scope.get(parent_call) {
            token.cancel();
        } else {
            // Nothing ran yet: later invokes start cancelled.
            let token = tokio_util::sync::CancellationToken::new();
            token.cancel();
            inner.inner_scope.insert(parent_call.to_owned(), token);
        }
    }
}

impl Kernel {
    /// The progress sink of a foreground tool call: updates are pushed to
    /// clients as `tool.progress`.
    pub(crate) fn tool_progress_sink(
        self: &Arc<Self>,
        branch: &BranchId,
        call_id: &str,
    ) -> ProgressSink {
        let kernel = self.clone();
        let branch = branch.clone();
        let call_id = call_id.to_owned();
        Arc::new(move |update| {
            let params = match update {
                Progress::Append(text) => {
                    json!({ "branch": branch, "call_id": call_id, "append": text })
                }
                Progress::Data(data) => {
                    json!({ "branch": branch, "call_id": call_id, "data": data })
                }
            };
            kernel.push("tool.progress", params);
        })
    }
}
