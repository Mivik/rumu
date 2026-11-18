//! The `@rumu:/tasks` part of the `@rumu` workspace: the kernel's runtime task
//! table, read-only.
//!
//! - `read /tasks` — every task of the current session;
//! - `read /tasks/<id>` — one task's status plus its buffered output, pageable
//!   with `offset` / `limit`.
//!
//! Tasks are runtime-only: a kernel restart loses running tasks, and their
//! old transcript entries remain ordinary notes/results.

use async_trait::async_trait;
use rumu_core::{OUTPUT_LIMIT, RUMU_WORKSPACE, TaskId, TaskInfo, ToolError, ToolOutput};

use super::time::iso_local;
use super::tools::slice_lines;
use super::vfs::{At, Node, Out};

/// The `@rumu` result prefix.
const WS: &str = RUMU_WORKSPACE;

/// `/tasks`: every task of the current session.
pub(super) struct Tasks;

#[async_trait]
impl Node for Tasks {
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
        Ok(format!("tasks ({session}):\n{}", lines.join("\n")).into())
    }
}

/// `/tasks/<id>`: one task's status and buffered output.
pub(super) struct Task;

#[async_trait]
impl Node for Task {
    async fn list(&self, at: &At<'_>) -> Out {
        self.read(at, None, None).await
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let id = TaskId::from(at.param("id"));
        let Some((info, output)) = at.cx.history().task(&id)? else {
            return Err(ToolError::model(format!("no task `{id}`")));
        };
        let text = slice_lines(task_view(&info, &output), offset, limit);
        Ok(ToolOutput::ok(capped_view(text.map_err(ToolError::model)?)))
    }
}

/// The unpaged `@rumu:/tasks/<id>` view: a header, the buffered output, and
/// the result once the task ended. Watch events count their line numbers in
/// it, so `offset` / `limit` land on the same lines.
pub(super) fn task_view(info: &TaskInfo, output: &str) -> String {
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
    text.push_str("\n\n");
    if output.is_empty() {
        text.push_str("(no output yet)");
    } else {
        text.push_str(output);
    }
    // The result renders images as their placeholder (task views are
    // text-only).
    if let Some(result) = &info.result {
        let rendered = result.rendered();
        if !rendered.is_empty() {
            text.push_str("\n\n--- result ---\n");
            text.push_str(&rendered);
        }
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
