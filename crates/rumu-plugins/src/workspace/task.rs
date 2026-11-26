//! The `@rumu:/tasks` part of the `@rumu` workspace: the kernel's runtime task
//! table, read-only.
//!
//! - `list /tasks` — every task of the current session;
//! - `read /tasks/<id>` — one task's summary: its status, the end of its
//!   output, and its result once it ended; `task.kill` cancels it,
//!   `task.wait` blocks the turn until it ends;
//! - `read /tasks/<id>/output` — its whole buffered output (then its result),
//!   pageable with `offset` / `limit`; watch events number their lines in it.
//!
//! The table is the kernel's live one; what a restart makes of it comes from
//! each task's record in the session journal (a task that was running is
//! `lost`, and its output is gone).

use async_trait::async_trait;
use rumu_core::{
    Cx, OUTPUT_LIMIT, RUMU_WORKSPACE, TaskId, TaskInfo, TaskKind, Timestamp, ToolError, ToolOutput,
    now,
};
use schemars::JsonSchema;
use serde::Deserialize;
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

/// `task.wait` arguments.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitArgs {
    #[allow(dead_code)]
    /// what you are waiting for, a few words; shown to the user while it waits
    status: Option<String>,
}

/// `/tasks`: every task of the current session.
pub(super) struct Tasks;

#[async_trait]
impl Node for Tasks {
    fn about(&self) -> &'static str {
        "this session's background tasks"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "This session's background tasks (`background: true` calls), oldest first, including \
         ones an earlier run of rumu left behind (a task that was still running then is \
         `lost`). One line each: address, status, how long it ran, what it runs; a sub-agent's \
         own tasks are indented under it. `read @rumu:/tasks/<id>` summarizes one (for a \
         sub-agent: what it is doing now); its `output` is the whole buffer."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let session = &at.cx.session;
        let tasks = at.cx.history().tasks(session)?;
        if tasks.is_empty() {
            return Ok(format!("no tasks in session {session}").into());
        }
        let mut lines = Vec::new();
        for t in tasks.iter().filter(|t| parent_of(&tasks, t).is_none()) {
            let mut line = task_line(t, now());
            if t.branch != at.cx.branch {
                line.push_str(&format!(
                    "  (on {})",
                    rumu_core::branch_address(&t.session, &t.branch)
                ));
            }
            lines.push(line);
            nested_lines(&tasks, t, 1, &mut lines);
        }
        Ok(lines.join("\n").into())
    }
}

/// The sub-agent task whose branch owns `t`, if any.
fn parent_of<'a>(tasks: &'a [TaskInfo], t: &TaskInfo) -> Option<&'a TaskInfo> {
    tasks
        .iter()
        .find(|p| matches!(&p.kind, TaskKind::Agent { child_branch: Some(c) } if *c == t.branch))
}

/// The tasks of the sub-agent `t` runs, oldest first.
fn children<'a>(tasks: &'a [TaskInfo], t: &TaskInfo) -> impl Iterator<Item = &'a TaskInfo> {
    let child = match &t.kind {
        TaskKind::Agent { child_branch } => child_branch.clone(),
        TaskKind::Tool { .. } => None,
    };
    tasks
        .iter()
        .filter(move |c| child.as_ref() == Some(&c.branch))
}

/// The lines of `t`'s sub-agent's tasks, indented `depth` levels, each
/// followed by its own.
fn nested_lines(tasks: &[TaskInfo], t: &TaskInfo, depth: usize, out: &mut Vec<String>) {
    for c in children(tasks, t) {
        out.push(format!("{}{}", "  ".repeat(depth), task_line(c, now())));
        nested_lines(tasks, c, depth + 1, out);
    }
}

/// One task as a list line: address, status, how long it ran, what it runs.
fn task_line(t: &TaskInfo, now: Timestamp) -> String {
    format!(
        "{WS}:/tasks/{}  {:<9} {:>6}  {}",
        t.id,
        t.status.label(),
        ran_for(t, now).unwrap_or_default(),
        t.what()
    )
}

/// How long `t` ran (so far, while it runs); `None` for a lost task,
/// whose end nobody saw.
fn ran_for(t: &TaskInfo, now: Timestamp) -> Option<String> {
    let end = match t.ended_at {
        Some(end) => end,
        None if t.status.is_running() => now,
        None => return None,
    };
    Some(duration((end - t.started_at).max(0) / 1000))
}

/// `secs` as `42s`, `7m`, `1h05m`.
fn duration(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
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
        Ok(summary(at.cx, &info, &output)?.into())
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let text = self.list(at).await?.content.text;
        let text = slice_lines(text, offset, limit).map_err(ToolError::model)?;
        Ok(ToolOutput::ok(capped_view(text)))
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[
            ActionSpec {
                name: "task.kill",
                about: "cancel it (a command's whole process tree) and report how it ended",
                args: rumu_core::tool_schema::<NoArgs>,
                approval: Approval::Never,
            },
            ActionSpec {
                name: "task.wait",
                about: "block until it ends, when you cannot go on without it; its note follows \
                        right after. A message from the user ends the wait",
                args: rumu_core::tool_schema::<WaitArgs>,
                approval: Approval::Never,
            },
        ]
    }

    /// `task.kill`: the kernel cancels it if this branch or one of its
    /// sub-agents owns it — the same cancellation as the web's `task.kill`.
    /// `task.wait`: the call holds the turn until the task ends (no
    /// foreground limit moves it to the background).
    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        let id = TaskId::from(at.param("id"));
        let failed = |e: rumu_core::Error| ToolError::model(e.to_string());
        if spec.name == "task.wait" {
            // `status` is for the user: the web shows it on the call's row.
            let _: WaitArgs = spec.parse(args)?;
            return Ok(match at.cx.wait_task(&id).await.map_err(failed)? {
                Some(info) => format!("task {id} finished: {}", info.status.label()),
                None => "the user sent a message; stopped waiting".to_owned(),
            }
            .into());
        }
        let NoArgs {} = spec.parse(args)?;
        let info = at.cx.kill_task(&id).await.map_err(failed)?;
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

/// A task's header, one `key: value` line each: what it is, its state,
/// when, and who started it.
fn header(info: &TaskInfo, now: Timestamp) -> String {
    let mut status = info.status.label().to_owned();
    if let Some(d) = ran_for(info, now) {
        let word = if info.status.is_running() {
            "for"
        } else {
            "after"
        };
        status.push_str(&format!(" {word} {d}"));
    }
    let mut lines = vec![
        format!("task: {}", info.id),
        format!("status: {status}"),
        format!("what: {}", info.what()),
        format!("started: {}", iso_local(info.started_at)),
    ];
    if let Some(ended) = info.ended_at {
        lines.push(format!("ended: {}", iso_local(ended)));
    }
    lines.push(format!(
        "owner: {}, call {}",
        rumu_core::branch_address(&info.session, &info.branch),
        info.call_id
    ));
    if let Some(child) = sub_agent(info) {
        lines.push(format!(
            "sub-agent: {} (`branch.send` on it messages it)",
            rumu_core::branch_address(&info.session, child)
        ));
    }
    lines.join("\n")
}

/// The branch of the sub-agent `info` runs, once it has one.
fn sub_agent(info: &TaskInfo) -> Option<&rumu_core::BranchId> {
    match &info.kind {
        TaskKind::Agent { child_branch } => child_branch.as_ref(),
        TaskKind::Tool { .. } => None,
    }
}

/// The result as text (images render as their placeholder), if any.
fn result_text(info: &TaskInfo) -> Option<String> {
    let rendered = info.result.as_ref()?.rendered();
    (!rendered.is_empty()).then_some(rendered)
}

/// The `@rumu:/tasks/<id>` summary: the header, then for a sub-agent its
/// own tasks and its latest nodes, otherwise the last [`SUMMARY_LINES`]
/// lines of output (numbered as in its `output`); then the start of the
/// result.
fn summary(cx: &Cx, info: &TaskInfo, output: &str) -> Result<String, ToolError> {
    let now = now();
    let mut text = header(info, now);
    let path = format!("{WS}:/tasks/{}/output", info.id);
    let lines: Vec<&str> = output.lines().collect();
    if let Some(child) = sub_agent(info) {
        let tasks = cx.history().tasks(&info.session)?;
        let mut nested = Vec::new();
        nested_lines(&tasks, info, 1, &mut nested);
        if !nested.is_empty() {
            text.push_str("\n\nits tasks:\n");
            text.push_str(&nested.join("\n"));
        }
        if let Some(tail) = super::session::branch_tail(cx, child)? {
            text.push_str(&format!(
                "\n\nits latest nodes (the whole transcript: `read {}`):\n",
                rumu_core::branch_address(&info.session, child)
            ));
            text.push_str(tail.trim_end());
        }
    } else if lines.is_empty() {
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
    Ok(text)
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
