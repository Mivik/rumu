//! The `@rumu:/task` part of the `@rumu` workspace: the kernel's runtime task
//! table, read-only.
//!
//! - `read /task` — every task of the current session;
//! - `read /task/<id>` — one task's status plus its buffered output, pageable
//!   with `offset` / `limit`.
//!
//! Tasks are runtime-only: a kernel restart loses running tasks, and their
//! old transcript entries remain ordinary notes/results.

use rumu_core::{
    Cx, OUTPUT_LIMIT, RUMU_WORKSPACE, TaskId, ToolError, ToolOutput, ToolResult, Utf8UnixPath,
};

use super::WorkspacePlugin;
use super::time::iso_local;
use super::tools::{slice_lines, unknown_rumu_path};

/// The `@rumu` result prefix.
const WS: &str = RUMU_WORKSPACE;

impl WorkspacePlugin {
    pub(super) async fn task_read(
        &self,
        abs: &Utf8UnixPath,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if abs.as_str() == "/task" {
            return self.task_listing(cx);
        }
        let id = abs
            .strip_prefix("/task")
            .ok()
            .map(Utf8UnixPath::as_str)
            .unwrap_or("");
        if id.is_empty() || id.contains('/') {
            return Err(unknown_rumu_path(abs));
        }
        let id = TaskId::from(id);
        let Some((info, output)) = cx.history().task(&id)? else {
            return Err(ToolError::model(format!("no task `{id}`")));
        };
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
            text.push_str(&output);
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
        let text = match slice_lines(text, offset, limit) {
            Ok(t) => t,
            Err(e) => return Err(ToolError::model(e)),
        };
        Ok(ToolOutput::ok(capped_view(text)))
    }

    fn task_listing(&self, cx: &Cx) -> ToolResult<ToolOutput> {
        let tasks = cx.history().tasks(&cx.session)?;
        if tasks.is_empty() {
            return Ok(format!("no tasks in session {}", cx.session).into());
        }
        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                format!(
                    "{WS}:/task/{}  {:<9} {}  ({})",
                    t.id,
                    t.status.label(),
                    t.label(),
                    iso_local(t.started_at)
                )
            })
            .collect();
        Ok(format!("tasks ({}):\n{}", cx.session, lines.join("\n")).into())
    }
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
