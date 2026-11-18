//! The `@rumu:/tasks` part of the `@rumu` workspace: the kernel's runtime task
//! table, read-only.
//!
//! - `list /tasks` — every task of the current session;
//! - `read /tasks/<id>` — one task's summary: its status, the end of its
//!   output, and its result once it ended; `task.kill` cancels it;
//! - `read /tasks/<id>/output` — its whole buffered output (then its result),
//!   pageable with `offset` / `limit`; watch events number their lines in it.
//!
//! Tasks are runtime-only: a kernel restart loses running tasks, and their
//! old transcript entries remain ordinary notes/results.

use async_trait::async_trait;
use rumu_core::{OUTPUT_LIMIT, RUMU_WORKSPACE, TaskId, TaskInfo, ToolError, ToolOutput};
use serde_json::Value;

use super::time::iso_local;
use super::tools::slice_lines;
use super::vfs::{ActionSpec, Approval, At, NoArgs, Node, Out};

/// The `@rumu` result prefix.
const WS: &str = RUMU_WORKSPACE;

/// Output lines a task's summary ends with.
const SUMMARY_LINES: usize = 20;

/// Characters of the result a summary shows.
const SUMMARY_RESULT: usize = 2_000;

/// `/tasks`: every task of the current session.
pub(super) struct Tasks;

#[async_trait]
impl Node for Tasks {
    fn about(&self) -> &'static str {
        "this session's background tasks"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "This session's background tasks (`background: true` calls), newest last. They live \
         only while rumu runs. `read @rumu:/tasks/<id>` summarizes one; its `output` is the \
         whole buffer."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let session = &at.cx.session;
        let tasks = at.cx.history().tasks(session)?;
        if tasks.is_empty() {
            return Ok(format!("no tasks in session {session}").into());
        }
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                format!(
                    "{WS}:/tasks/{}  {:<9} {}  ({})",
                    t.id,
                    t.status.label(),
                    t.label(),
                    iso_local(t.started_at)
                )
            })
            .collect();
        Ok(lines.join("\n").into())
    }
}

/// The task `at` names, with its buffered output.
fn task_of(at: &At<'_>) -> Result<(TaskInfo, String), ToolError> {
    let id = TaskId::from(at.param("id"));
    at.cx
        .history()
        .task(&id)?
        .ok_or_else(|| ToolError::model(format!("no task `{id}`")))
}

/// `/tasks/<id>`: one task's summary.
pub(super) struct Task;

#[async_trait]
impl Node for Task {
    fn about(&self) -> &'static str {
        "one task: its status, the end of its output, its result"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (info, output) = task_of(at)?;
        Ok(summary(&info, &output).into())
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let text = self.list(at).await?.content.text;
        let text = slice_lines(text, offset, limit).map_err(ToolError::model)?;
        Ok(ToolOutput::ok(capped_view(text)))
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[ActionSpec {
            name: "task.kill",
            about: "cancel it (a command's whole process tree) and report how it ended",
            args: rumu_core::tool_schema::<NoArgs>,
            approval: Approval::Never,
        }]
    }

    /// `task.kill`: the kernel cancels it if this branch or one of its
    /// sub-agents owns it — the same cancellation as the web's `task.kill`.
    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        let NoArgs {} = spec.parse(args)?;
        let id = TaskId::from(at.param("id"));
        let info = at
            .cx
            .kill_task(&id)
            .await
            .map_err(|e| ToolError::model(e.to_string()))?;
        Ok(format!("task {id}: {}", info.status.label()).into())
    }
}

/// `/tasks/<id>/output`: its whole buffered output, then its result.
pub(super) struct TaskOutput;

#[async_trait]
impl Node for TaskOutput {
    fn about(&self) -> &'static str {
        "its whole buffered output (then its result); page with `offset`/`limit`"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (info, output) = task_of(at)?;
        Ok(output_view(&info, &output).into())
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let text = self.list(at).await?.content.text;
        let text = slice_lines(text, offset, limit).map_err(ToolError::model)?;
        Ok(ToolOutput::ok(capped_view(text)))
    }
}

/// A task's header: what it is, its state and when.
fn header(info: &TaskInfo) -> String {
    let mut text = format!(
        "task {} · {} · {} · started {}\nowner branch {} · call {}",
        info.id,
        info.status.label(),
        info.label(),
        iso_local(info.started_at),
        info.branch,
        info.call_id
    );
    if let Some(ended) = info.ended_at {
        text.push_str(&format!(" · ended {}", iso_local(ended)));
    }
    text
}

/// The result as text (images render as their placeholder), if any.
fn result_text(info: &TaskInfo) -> Option<String> {
    let rendered = info.result.as_ref()?.rendered();
    (!rendered.is_empty()).then_some(rendered)
}

/// The `@rumu:/tasks/<id>` summary: the header, the last
/// [`SUMMARY_LINES`] lines of output (numbered as in its `output`), and
/// the start of the result.
fn summary(info: &TaskInfo, output: &str) -> String {
    let mut text = header(info);
    let path = format!("{WS}:/tasks/{}/output", info.id);
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        text.push_str("\n\n(no output yet)");
    } else {
        let from = lines.len().saturating_sub(SUMMARY_LINES);
        let shown = if from == 0 {
            format!("\n\noutput ({} lines):", lines.len())
        } else {
            format!(
                "\n\noutput, lines {}–{} of {} (all of it: `{path}`):",
                from + 1,
                lines.len(),
                lines.len()
            )
        };
        text.push_str(&shown);
        text.push('\n');
        text.push_str(&lines[from..].join("\n"));
    }
    if let Some(result) = result_text(info) {
        text.push_str("\n\n--- result ---\n");
        if result.chars().count() > SUMMARY_RESULT {
            text.extend(result.chars().take(SUMMARY_RESULT));
            text.push_str(&format!("\n… (the rest: `{path}`)"));
        } else {
            text.push_str(&result);
        }
    }
    text
}

/// The unpaged `@rumu:/tasks/<id>/output`: the buffered output, then the
/// result once the task ended. Watch events number their lines in it, so
/// `offset` / `limit` land on the same lines.
pub(super) fn output_view(info: &TaskInfo, output: &str) -> String {
    let mut text = if output.is_empty() {
        "(no output yet)".to_owned()
    } else {
        output.trim_end_matches('\n').to_owned()
    };
    if let Some(result) = result_text(info) {
        text.push_str("\n\n--- result ---\n");
        text.push_str(&result);
    }
    text
}

/// Cap the view like any tool output, but point at `offset` / `limit` paging
/// instead of claiming completeness. Stays within [`OUTPUT_LIMIT`] so the
/// boundary cap leaves it alone.
fn capped_view(text: String) -> String {
    let total = text.chars().count();
    if total <= OUTPUT_LIMIT {
        return text;
    }
    let note =
        format!("\n… [truncated, {total} chars total; pass `offset`/`limit` to page through it]");
    let keep = OUTPUT_LIMIT.saturating_sub(note.chars().count());
    let head: String = text.chars().take(keep).collect();
    format!("{head}{note}")
}
