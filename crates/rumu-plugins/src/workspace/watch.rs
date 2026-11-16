//! `bash` watch mode: a background bash whose new output reaches its owner
//! branch while it runs. Chunks are batched (a quiet gap or a longest wait)
//! and posted through the plugin's inbox as waking notes. The batch still
//! pending when the command exits is not posted: it is the task's result, so
//! the kernel's completion note carries it once. Dropping the run (`task_kill`,
//! an abort) drops the batching with it, so nothing is posted afterwards.

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use rumu_core::{
    BranchKind, Content, Cx, Post, RUMU_WORKSPACE, Structured, TaskId, ToolError, ToolResult,
    truncate_middle,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;
use tracing::warn;

use super::WorkspacePlugin;
use super::task::task_view;

/// When a batch is flushed: after `quiet` without new output, or `max` after
/// its first chunk, whichever comes first.
#[derive(Debug, Clone, Copy)]
pub(super) struct WatchTiming {
    pub quiet: Duration,
    pub max: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            quiet: Duration::from_secs(1),
            max: Duration::from_secs(5),
        }
    }
}

/// Characters of a batch shown inline; a longer one keeps its start and end
/// and points at its lines in `@rumu:/tasks/<id>`.
const INLINE_LIMIT: usize = 2_000;

/// Payload of a watch event note (`workspace.watch` v1): the batch's lines in
/// the `@rumu:/tasks/<id>` view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchEvent {
    pub task: TaskId,
    pub from: usize,
    pub to: usize,
    /// Whether the note cut the middle of the batch.
    pub truncated: bool,
}

impl Structured for WatchEvent {
    const TAG: &'static str = "workspace.watch";
    const V: u32 = 1;
}

/// Output not yet posted, and when it started and last grew.
#[derive(Default)]
pub(super) struct Batch {
    text: String,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Batch {
    pub(super) fn push(&mut self, chunk: &str, now: Instant) {
        if chunk.is_empty() {
            return;
        }
        self.text.push_str(chunk);
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// When the pending text is due; `None` when there is none.
    fn due(&self, timing: WatchTiming) -> Option<Instant> {
        Some((self.last? + timing.quiet).min(self.first? + timing.max))
    }

    pub(super) fn take(&mut self) -> String {
        self.first = None;
        self.last = None;
        std::mem::take(&mut self.text)
    }
}

/// A checked watch: where its events go.
pub(super) struct Watch {
    task: TaskId,
    inbox: std::sync::Arc<dyn rumu_core::Inbox>,
    timing: WatchTiming,
}

impl WorkspacePlugin {
    /// Check that this `bash` call may watch: a model's background call on a
    /// branch that is not a sub-agent (it waits for its own tasks, and a
    /// watch may never end), with the task the kernel started for it.
    pub(super) fn watch_for(&self, cx: &Cx) -> ToolResult<Watch> {
        let call = cx
            .call()
            .filter(|c| c.background)
            .ok_or_else(|| ToolError::model("`watch` needs `background: true`"))?;
        let inbox = self
            .inbox
            .get()
            .cloned()
            .ok_or_else(|| ToolError::model("`watch` is not available on this host"))?;
        let (_, _, branches) = cx.history().session_tree(&cx.session)?;
        if branches
            .iter()
            .any(|b| b.id == cx.branch && matches!(b.kind, BranchKind::Agent { .. }))
        {
            return Err(ToolError::model(
                "`watch` is not available in a sub-agent: it waits for its own tasks, and a \
                 watch may never end; run the command without `watch`",
            ));
        }
        let task = cx
            .history()
            .tasks(&cx.session)?
            .into_iter()
            .find(|t| t.call_id == call.id && t.branch == cx.branch)
            .map(|t| t.id)
            .ok_or_else(|| ToolError::model("`watch`: no task runs this call"))?;
        Ok(Watch {
            task,
            inbox,
            timing: self.watch_timing,
        })
    }
}

impl Watch {
    /// Drive `exec` to its end, posting each batch of `batch` once it is due.
    /// `notify` is signalled after every push. What is still pending when
    /// `exec` ends stays in `batch`.
    pub(super) async fn run<T>(
        &self,
        exec: impl Future<Output = T>,
        batch: &Mutex<Batch>,
        notify: &Notify,
        cx: &Cx,
    ) -> T {
        tokio::pin!(exec);
        loop {
            let due = batch.lock().unwrap().due(self.timing);
            // Disabled when nothing is pending (the deadline is never polled).
            let sleep = tokio::time::sleep_until(due.unwrap_or_else(Instant::now));
            tokio::select! {
                biased;
                out = &mut exec => return out,
                _ = notify.notified() => {}
                _ = sleep, if due.is_some() => {
                    let text = batch.lock().unwrap().take();
                    self.post(&text, cx);
                }
            }
        }
    }

    /// Post one batch as a waking note.
    fn post(&self, text: &str, cx: &Cx) {
        let lines = text.lines().count().max(1);
        // The batch is the end of the task's buffer: count back from the
        // view's last line.
        let to = match cx.history().task(&self.task) {
            Ok(Some((info, output))) => task_view(&info, &output).lines().count(),
            _ => lines,
        };
        let note = event_note(&self.task, text, to.saturating_sub(lines) + 1, to);
        let post = Post {
            effects: vec![note],
            wake: true,
        };
        if let Err(e) = self.inbox.post(&cx.branch, post) {
            warn!(task = %self.task, error = %e, "watch event post failed");
        }
    }
}

/// `watch <id>: +N lines (L a–b)` and the batch, or its start and end plus a
/// pointer at its lines when it is long.
fn event_note(task: &TaskId, text: &str, from: usize, to: usize) -> Content {
    let body = text.trim_end_matches('\n');
    let lines = to + 1 - from;
    let truncated = body.chars().count() > INLINE_LIMIT;
    let mut note = format!("watch {task}: +{lines} lines (L {from}–{to})\n");
    if truncated {
        note.push_str(&truncate_middle(body.to_owned(), INLINE_LIMIT));
        note.push_str(&format!(
            "\n(whole batch: read {RUMU_WORKSPACE}:/tasks/{task} offset={from} limit={lines})"
        ));
    } else {
        note.push_str(body);
    }
    Content::note_with(
        note,
        &WatchEvent {
            task: task.clone(),
            from,
            to,
            truncated,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeInbox, FakeRuntime};
    use rumu_core::{
        Branch, BranchId, BranchStatus, Call, NodeId, Plugin, Progress, ProgressSink, SessionId,
        SessionInfo, TaskInfo, TaskKind, TaskStatus, Workspace,
    };
    use serde_json::json;
    use std::sync::Arc;

    const MS: Duration = Duration::from_millis(1);

    fn timing() -> WatchTiming {
        WatchTiming {
            quiet: 150 * MS,
            max: 400 * MS,
        }
    }

    #[test]
    fn batch_is_due_after_a_quiet_gap_or_the_longest_wait() {
        let t = Instant::now();
        let mut batch = Batch::default();
        assert_eq!(batch.due(timing()), None);
        batch.push("a\n", t);
        assert_eq!(batch.due(timing()), Some(t + 150 * MS));
        batch.push("b\n", t + 100 * MS);
        assert_eq!(batch.due(timing()), Some(t + 250 * MS));
        // Steady output: the longest wait caps it.
        batch.push("c\n", t + 300 * MS);
        assert_eq!(batch.due(timing()), Some(t + 400 * MS));
        assert_eq!(batch.take(), "a\nb\nc\n");
        assert_eq!(batch.due(timing()), None);
    }

    #[test]
    fn long_batches_keep_both_ends_and_point_at_their_lines() {
        let task = TaskId::from("t-1");
        let text: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        let note = event_note(&task, &text, 4, 503);
        assert!(
            note.text
                .starts_with("watch t-1: +500 lines (L 4–503)\nline 1\n"),
            "{}",
            note.text
        );
        assert!(
            note.text.contains("line 500\n(whole batch: "),
            "{}",
            note.text
        );
        assert!(
            note.text
                .ends_with("read @rumu:/tasks/t-1 offset=4 limit=500)"),
            "{}",
            note.text
        );
        assert!(note.text.chars().count() < INLINE_LIMIT + 200);
        let data: WatchEvent = note.decode().unwrap();
        assert!(data.truncated);

        let short = event_note(&task, "ok\n", 7, 7);
        assert_eq!(short.text, "watch t-1: +1 lines (L 7–7)\nok");
        assert!(!short.decode::<WatchEvent>().unwrap().truncated);
    }

    /// A plugin over `root` with a fake inbox, and a background call's `Cx`
    /// whose task (`t-w`) buffers its progress like the kernel's.
    fn setup(
        root: &std::path::Path,
        kind: BranchKind,
    ) -> (
        Arc<WorkspacePlugin>,
        Arc<FakeRuntime>,
        Arc<FakeInbox>,
        Cx,
        tempfile::TempDir,
    ) {
        let home_dir = tempfile::tempdir().unwrap();
        let home = home_dir.path();
        crate::testutil::main_deck(home);
        let decks = rumu_cards::Decks::load(home).unwrap();
        let mut plugin = WorkspacePlugin::new(home, decks, Default::default());
        plugin.watch_timing = timing();
        let inbox = FakeInbox::arc();
        plugin.attach(inbox.clone());
        let host = FakeRuntime::arc();
        {
            let mut state = host.state.lock().unwrap();
            state.workspaces = vec![Workspace {
                name: "proj".into(),
                ssh: None,
                root: root.to_string_lossy().as_ref().into(),
                read_only: false,
            }];
            state.default_workspace = Some("proj".into());
        }
        let session = SessionId::new();
        let branch = BranchId::new();
        let info = SessionInfo {
            id: session.clone(),
            title: String::new(),
            active_branch: branch.clone(),
            created_at: 0,
            updated_at: 0,
            touched_at: 0,
            seen_at: None,
            assistant: crate::testutil::TEST_ASSISTANT.into(),
            project: None,
        };
        let row = Branch {
            id: branch.clone(),
            session: session.clone(),
            head: NodeId::from("N1"),
            forked_from: None,
            kind,
            status: BranchStatus::Active,
            created_at: 0,
        };
        host.session_trees
            .lock()
            .unwrap()
            .push((session.clone(), info, vec![], vec![row]));
        host.tasks.lock().unwrap().push((
            TaskInfo {
                id: TaskId::from("t-w"),
                session: session.clone(),
                branch: branch.clone(),
                call_id: "call-1".into(),
                kind: TaskKind::Tool {
                    name: "bash".into(),
                    args_summary: "watch".into(),
                },
                summary: "watch".into(),
                status: TaskStatus::Running,
                started_at: 0,
                ended_at: None,
                truncated: false,
                is_error: false,
                result: None,
                data: None,
            },
            String::new(),
        ));
        let sink: ProgressSink = {
            let host = host.clone();
            Arc::new(move |update| {
                if let Progress::Append(text) = update {
                    host.tasks.lock().unwrap()[0].1.push_str(&text);
                }
            })
        };
        let cx = Cx::new(
            session,
            branch,
            Some(Call::new("call-1".into(), None, true, sink)),
            Some("workspace".into()),
            0,
            host.clone(),
        );
        (Arc::new(plugin), host, inbox, cx, home_dir)
    }

    fn texts(inbox: &FakeInbox) -> Vec<String> {
        inbox
            .posts()
            .into_iter()
            .map(|(_, post)| {
                assert!(post.wake);
                post.effects[0].text.clone()
            })
            .collect()
    }

    /// Output separated by a quiet gap arrives as separate waking notes whose
    /// line range reads back the same lines from `@rumu:/tasks/<id>`; what
    /// is pending at exit is the result, not a note.
    #[tokio::test]
    async fn watch_posts_batches_and_leaves_the_tail_to_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "echo one; echo two; sleep 0.6; echo three; sleep 0.6; echo tail";
        let out = plugin
            .call_tool("bash", json!({ "command": script, "watch": true }), &cx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        assert_eq!(out.content.text, "exit code: 0\ntail\n");
        let posts = texts(&inbox);
        assert_eq!(
            posts,
            [
                "watch t-w: +2 lines (L 4–5)\none\ntwo",
                "watch t-w: +1 lines (L 6–6)\nthree",
            ]
        );
        // The pointer lines are the task view's lines.
        let read = plugin
            .call_tool(
                "read",
                json!({ "path": "@rumu:/tasks/t-w", "offset": 4, "limit": 2 }),
                &cx,
            )
            .await
            .unwrap();
        assert_eq!(read.content.text, "one\ntwo");
    }

    /// Steady output is still flushed at the longest wait.
    #[tokio::test]
    async fn watch_flushes_steady_output_at_the_longest_wait() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "for i in $(seq 30); do echo $i; sleep 0.05; done";
        let out = plugin
            .call_tool("bash", json!({ "command": script, "watch": true }), &cx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        let posts = texts(&inbox);
        assert!(posts.len() >= 2, "{posts:?}");
        // Nothing is lost or repeated between the notes and the result.
        let mut seen: Vec<String> = posts
            .iter()
            .flat_map(|p| p.lines().skip(1).map(str::to_owned).collect::<Vec<_>>())
            .collect();
        seen.extend(out.content.text.lines().skip(1).map(str::to_owned));
        let want: Vec<String> = (1..=30).map(|i| i.to_string()).collect();
        assert_eq!(seen, want);
    }

    /// Once the run is dropped (`task_kill`), nothing more is posted.
    #[tokio::test]
    async fn watch_posts_nothing_after_the_task_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let args =
            json!({ "command": "while true; do echo tick; sleep 0.05; done", "watch": true });
        let handle = tokio::spawn(async move {
            let _ = plugin.call_tool("bash", args, &cx).await;
        });
        for _ in 0..100 {
            if !inbox.posts().is_empty() {
                break;
            }
            tokio::time::sleep(20 * MS).await;
        }
        assert!(!inbox.posts().is_empty(), "no event before the kill");
        handle.abort();
        let _ = handle.await;
        let posted = inbox.posts().len();
        tokio::time::sleep(timing().max * 2).await;
        assert_eq!(inbox.posts().len(), posted);
    }

    /// `watch` is refused in a sub-agent and without `background`.
    #[tokio::test]
    async fn watch_is_refused_in_sub_agents_and_in_the_foreground() {
        let dir = tempfile::tempdir().unwrap();
        let agent = BranchKind::Agent {
            task: "t".into(),
            parent_branch: BranchId::new(),
            call_id: "c".into(),
            via_plugin: false,
            inherit: Default::default(),
        };
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), agent);
        let args = json!({ "command": "echo hi", "watch": true });
        let out = plugin.call_tool("bash", args.clone(), &cx).await.unwrap();
        assert!(out.is_error);
        assert!(
            out.content.text.contains("sub-agent"),
            "{}",
            out.content.text
        );

        let (plugin, host, _, _, _home2) = setup(dir.path(), BranchKind::Main);
        let out = plugin.call_tool("bash", args, &host.cx()).await.unwrap();
        assert!(out.is_error);
        assert!(
            out.content.text.contains("needs `background: true`"),
            "{}",
            out.content.text
        );
        assert!(inbox.posts().is_empty());
    }
}
