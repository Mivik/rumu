use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{BranchId, Content, RUMU_WORKSPACE, SessionId, Structured, Tagged, Timestamp};

/// A kernel task id: `t-` plus 8 random hex digits. Random (not sequential)
/// because ids are written into persisted notes, and a restarted kernel must
/// not hand a fresh task an id an old transcript already uses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ts_rs::TS)]
#[serde(transparent)]
#[ts(export)]
pub struct TaskId(pub String);

impl TaskId {
    /// A fresh random id (`t-1a2b3c4d`). Callers that must stay unique within
    /// a live table regenerate on collision.
    pub fn new() -> Self {
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        Self(format!("t-{}", &uuid[..8]))
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// What a task runs: a plugin tool, or a background sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum TaskKind {
    Tool {
        name: String,
        args_summary: String,
    },
    /// A background sub-agent. The child branch is created a moment after the
    /// task starts (it may first wait for an agent permit), so it is absent
    /// until then.
    Agent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        child_branch: Option<BranchId>,
    },
}

impl TaskKind {
    /// One-line label: `bash: rm -rf dist`, `agent: find the config`. Agent
    /// labels use the task's `summary`.
    pub fn label(&self, summary: &str) -> String {
        match self {
            Self::Tool { name, args_summary } => {
                if args_summary.is_empty() {
                    name.clone()
                } else {
                    format!("{name}: {args_summary}")
                }
            }
            Self::Agent { .. } => format!("agent: {}", agent_label(summary)),
        }
    }
}

/// A sub-agent's task as a label: its first line, cut to 80 characters
/// (marked with `…` when anything was left out).
pub fn agent_label(task: &str) -> String {
    let task = task.trim();
    let first = task.lines().next().unwrap_or_default().trim_end();
    let mut label: String = first.chars().take(80).collect();
    if label.len() < task.len() {
        label.push('…');
    }
    label
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskStatus {
    Running,
    Done,
    Failed,
    Cancelled,
    /// A kernel restart killed it before it finished: the work is gone, and
    /// whatever it wrote to its output buffer with it.
    Lost,
}

impl TaskStatus {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// The word used in completion notes and `@rumu:/tasks` views.
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "ok",
            Self::Failed => "error",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }
}

/// A read-only snapshot of one kernel task. The table itself is runtime state,
/// but each task also leaves a record in its session's journal, so after a
/// restart a task that was running comes back as [`TaskStatus::Lost`] instead
/// of vanishing (its output buffer is not kept).
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TaskInfo {
    pub id: TaskId,
    pub session: SessionId,
    /// The branch that owns the task (the branch whose tool call started it).
    pub branch: BranchId,
    /// The model/plugin call id that started it.
    pub call_id: String,
    pub kind: TaskKind,
    /// Short human label, e.g. the bash command or the agent's task.
    pub summary: String,
    pub status: TaskStatus,
    pub started_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<Timestamp>,
    /// The output buffer dropped earlier text (about 1 MiB is kept).
    #[serde(default)]
    pub truncated: bool,
    /// The final result is an error (or the task failed / was cancelled).
    #[serde(default)]
    pub is_error: bool,
    /// The final result, once the task ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Content>,
    /// The latest `Progress::Data` payload reported by the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Tagged>,
}

impl TaskInfo {
    /// One-line label: `bash: rm -rf dist`, `agent: find the config`.
    pub fn label(&self) -> String {
        self.kind.label(&self.summary)
    }
}

/// Structured payload of a task-completion note (`task.done` v1), carried in
/// the note's `content.data`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TaskDone {
    pub id: TaskId,
    pub status: TaskStatus,
    pub kind: TaskKind,
    #[serde(default)]
    pub is_error: bool,
}

impl Structured for TaskDone {
    const TAG: &'static str = "task.done";
    const V: u32 = 1;
}

/// The line appended with a task's result when it was truncated: the buffered
/// output stays readable through `@rumu:/tasks/<id>/output`, paging with
/// `offset` / `limit` when it exceeds the tool-output cap.
pub fn task_output_hint(id: &TaskId) -> String {
    format!(
        "read {RUMU_WORKSPACE}:/tasks/{id}/output for the whole output (page with `offset`/`limit`)"
    )
}
