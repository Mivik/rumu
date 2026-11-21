//! `bash` watch mode: a background bash whose new output lines reach its
//! owner branch while it runs, for a bounded lifetime. Lines are scanned as
//! they arrive (`match` picks the event lines, `until` ends the watch),
//! batched (a quiet gap or a longest wait) and posted through the plugin's
//! inbox as notes, under a token-bucket rate limit that defers batches
//! rather than dropping them. With `silence`, a stretch without events is one. The batch still pending
//! when the watch ends is
//! not posted: it is the task's result, so the kernel's completion note
//! carries it once. Dropping the run (`task.kill`, an abort) drops the
//! batching with it, so nothing is posted afterwards.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use regex::Regex;
use rumu_core::{
    BranchKind, Content, Cx, Inbox, Post, RUMU_WORKSPACE, Structured, TaskId, ToolError,
    ToolResult, truncate_middle,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::time::Instant;
use tracing::warn;

use super::WorkspacePlugin;

/// `bash`'s `watch` argument (`{}` watches every line).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct WatchArgs {
    /// regex: only matching lines are events (all output still lands in
    /// `@rumu:/tasks/<id>/output`)
    #[serde(rename = "match")]
    filter: Option<String>,
    /// regex: a matching line is the last event; the command is then killed
    until: Option<String>,
    /// seconds: an event when no new output (matching lines with `match`)
    /// came for this long
    silence: Option<u64>,
}

/// A watch's lifetime (`bash`'s `timeout` in watch mode), in seconds.
pub(super) const LIFETIME_DEFAULT: u64 = 1800;
pub(super) const LIFETIME_MAX: u64 = 86_400;

/// Posts the rate limit lets through in a burst.
const BUCKET: u32 = 10;

/// Characters of a batch shown inline; a longer one keeps its start and end
/// and points at its lines in `@rumu:/tasks/<id>/output`.
const INLINE_LIMIT: usize = 2_000;

/// Characters a batch keeps of its start, and of its end: within
/// [`INLINE_LIMIT`] together with the omission marker.
const KEEP: usize = INLINE_LIMIT * 9 / 20;

/// Bytes of one line kept for matching; the rest of a longer line is only
/// in the task's output.
const LINE_MAX: usize = 64 * 1024;

/// When a batch is flushed (after `quiet` without new event lines, or `max`
/// after its first, whichever comes first) and the rate limit: one post per
/// `refill` beyond a burst of [`BUCKET`]; batches deferred for longer than
/// `flood` end the watch. `second` is the unit of `silence`.
#[derive(Debug, Clone, Copy)]
pub(super) struct WatchTiming {
    pub quiet: Duration,
    pub max: Duration,
    pub refill: Duration,
    pub flood: Duration,
    pub second: Duration,
}

impl Default for WatchTiming {
    fn default() -> Self {
        Self {
            quiet: Duration::from_secs(1),
            max: Duration::from_secs(5),
            refill: Duration::from_secs(2),
            flood: Duration::from_secs(30),
            second: Duration::from_secs(1),
        }
    }
}

/// Payload of a watch event note (`workspace.watch` v1): the batch's lines in
/// `@rumu:/tasks/<id>/output`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WatchEvent {
    pub task: TaskId,
    pub from: usize,
    pub to: usize,
    /// Whether the note cut the middle of the batch.
    pub truncated: bool,
    /// Event lines in the batch (0 in notes written before it existed).
    #[serde(default)]
    pub count: usize,
    /// Whether the watch has a `match` filter (the lines are matching ones).
    #[serde(default)]
    pub matching: bool,
}

impl Structured for WatchEvent {
    const TAG: &'static str = "workspace.watch";
    const V: u32 = 1;
}

/// Payload of a watch's `silence` note (`workspace.watch_silence` v1): no
/// event came for `secs` seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WatchSilence {
    pub task: TaskId,
    pub secs: u64,
    /// Whether the watch has a `match` filter (no matching line came).
    pub matching: bool,
}

impl Structured for WatchSilence {
    const TAG: &'static str = "workspace.watch_silence";
    const V: u32 = 1;
}

/// Event lines not yet posted, numbered by their place in the task's output
/// (1-based), and when the batch started and last grew. Only its start and
/// end are kept, as much as a note shows.
#[derive(Default)]
struct Batch {
    head: Vec<(usize, String)>,
    tail: VecDeque<(usize, String)>,
    head_chars: usize,
    tail_chars: usize,
    count: usize,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Batch {
    fn push(&mut self, n: usize, text: &str, now: Instant) {
        self.touch(now);
        self.count += 1;
        let chars = text.chars().count() + 1;
        if self.head_chars < KEEP {
            self.head_chars += chars;
            self.head.push((n, text.to_owned()));
            return;
        }
        self.tail_chars += chars;
        self.tail.push_back((n, text.to_owned()));
        while self.tail_chars > KEEP && self.tail.len() > 1 {
            if let Some((_, t)) = self.tail.pop_front() {
                self.tail_chars -= t.chars().count() + 1;
            }
        }
    }

    /// Mark new pending output (a line, or an unfinished one).
    fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    /// When the pending output is due; `None` when there is none.
    fn due(&self, timing: WatchTiming) -> Option<Instant> {
        Some((self.last? + timing.quiet).min(self.first? + timing.max))
    }
}

/// The scan of the command's output so far.
#[derive(Default)]
struct Scan {
    /// The unfinished last line, up to [`LINE_MAX`] bytes.
    partial: String,
    /// Bytes of `partial` already posted (without `match`, a quiet gap posts
    /// an unfinished line).
    shown: usize,
    /// Finished lines in the task's output.
    seen: usize,
    /// The output ends inside a line.
    open: bool,
    batch: Batch,
    /// The line `until` matched: the watch is over.
    until: Option<usize>,
    /// When the last event line came (`None`: none yet), and whether the
    /// silence since then was posted.
    heard: Option<Instant>,
    hushed: bool,
}

impl Scan {
    fn heard(&mut self, now: Instant) {
        self.heard = Some(now);
        self.hushed = false;
    }
}

/// A batch rendered with its task-view line numbers.
struct Rendered {
    body: String,
    count: usize,
    from: usize,
    to: usize,
    truncated: bool,
}

/// The rate limit: up to [`BUCKET`] tokens, one more per `refill`.
struct Bucket {
    tokens: u32,
    since: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self {
            tokens: BUCKET,
            since: now,
        }
    }

    /// Spend a token, or tell when the next one comes.
    fn take(&mut self, now: Instant, refill: Duration) -> Result<(), Instant> {
        if self.tokens >= BUCKET {
            self.since = now;
        } else {
            let earned = (now - self.since).as_nanos() / refill.as_nanos().max(1);
            let earned = earned.min(u128::from(BUCKET - self.tokens)) as u32;
            self.tokens += earned;
            self.since = if self.tokens >= BUCKET {
                now
            } else {
                self.since + refill * earned
            };
        }
        if self.tokens == 0 {
            return Err(self.since + refill);
        }
        self.tokens -= 1;
        Ok(())
    }
}

/// Why a watch ended before its command exited.
enum Ending {
    Until,
    Expired,
    Flooded,
}

/// A checked watch: what it picks and where its events go.
pub(super) struct Watch {
    task: TaskId,
    /// ` (<description>)`, or empty.
    label: String,
    inbox: Arc<dyn Inbox>,
    timing: WatchTiming,
    filter: Option<Regex>,
    until: Option<Regex>,
    lifetime: Duration,
    /// The `silence` argument, and as a duration.
    silence: Option<(u64, Duration)>,
    scan: Mutex<Scan>,
    notify: Notify,
}

impl WorkspacePlugin {
    /// Check that this `bash` call may watch: a model's background call on a
    /// branch that is not a sub-agent (it waits for its own tasks), with the
    /// task the kernel started for it and valid patterns. `timeout` is the
    /// watch's lifetime.
    pub(super) fn watch_for(
        &self,
        cx: &Cx,
        args: WatchArgs,
        timeout: Option<u64>,
        description: Option<&str>,
    ) -> ToolResult<Watch> {
        let call = cx
            .call()
            .filter(|c| c.background)
            .ok_or_else(|| ToolError::model("`watch` needs `background: true`"))?;
        let regex = |name: &str, re: Option<String>| {
            re.map(|re| Regex::new(&re))
                .transpose()
                .map_err(|e| ToolError::model(format!("`watch.{name}`: {e}")))
        };
        let filter = regex("match", args.filter)?;
        let until = regex("until", args.until)?;
        let silence = match args.silence {
            Some(0) => return Err(ToolError::model("`watch.silence` must be at least 1")),
            s => s.map(|s| {
                let d = self.watch_timing.second * u32::try_from(s).unwrap_or(u32::MAX);
                (s, d)
            }),
        };
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
                "`watch` is not available in a sub-agent: it waits for its own tasks; run the \
                 command without `watch`",
            ));
        }
        let task = cx
            .history()
            .tasks(&cx.session)?
            .into_iter()
            .find(|t| t.call_id == call.id && t.branch == cx.branch)
            .map(|t| t.id)
            .ok_or_else(|| ToolError::model("`watch`: no task runs this call"))?;
        let lifetime = timeout.unwrap_or(LIFETIME_DEFAULT).clamp(1, LIFETIME_MAX);
        Ok(Watch {
            task,
            label: description
                .filter(|d| !d.trim().is_empty())
                .map(|d| format!(" ({})", d.trim()))
                .unwrap_or_default(),
            inbox,
            timing: self.watch_timing,
            filter,
            until,
            lifetime: Duration::from_secs(lifetime),
            silence,
            scan: Mutex::default(),
            notify: Notify::new(),
        })
    }
}

/// Append `text` to a line, keeping at most [`LINE_MAX`] bytes.
fn append_capped(line: &mut String, text: &str) {
    let room = LINE_MAX.saturating_sub(line.len());
    let mut end = text.len().min(room);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    line.push_str(&text[..end]);
}

impl Watch {
    /// Scan a chunk of the command's output (it is already in the task's
    /// buffer).
    pub(super) fn push(&self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut scan = self.scan.lock().unwrap();
        let s = &mut *scan;
        if s.until.is_some() {
            return;
        }
        s.open = !chunk.ends_with('\n');
        let mut rest = chunk;
        while let Some(i) = rest.find('\n') {
            append_capped(&mut s.partial, &rest[..i]);
            rest = &rest[i + 1..];
            s.seen += 1;
            let line = std::mem::take(&mut s.partial);
            if self.scan_line(s, s.seen, &line, now) {
                break;
            }
        }
        if s.until.is_none() && !rest.is_empty() {
            append_capped(&mut s.partial, rest);
            if self.filter.is_none() {
                s.batch.touch(now);
                s.heard(now);
            }
        }
        drop(scan);
        self.notify.notify_one();
    }

    /// Batch one finished line `n` if it is an event; `true` when `until`
    /// matched it.
    fn scan_line(&self, s: &mut Scan, n: usize, line: &str, now: Instant) -> bool {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let rest = line.get(std::mem::take(&mut s.shown)..).unwrap_or("");
        if self.until.as_ref().is_some_and(|re| re.is_match(line)) {
            s.batch.push(n, rest, now);
            s.until = Some(n);
            return true;
        }
        let event = match &self.filter {
            Some(re) => re.is_match(line),
            None => !rest.is_empty(),
        };
        if event {
            s.batch.push(n, rest, now);
            s.heard(now);
        }
        false
    }

    /// Drive `exec` to its end, posting each batch once it is due and the
    /// rate limit allows. `Err` carries the result text of a watch that
    /// ended first (`until`, its lifetime, a flood); `exec` is dropped then,
    /// which kills the command.
    pub(super) async fn run<T>(&self, exec: impl Future<Output = T>, cx: &Cx) -> Result<T, String> {
        tokio::pin!(exec);
        let start = Instant::now();
        let expiry = tokio::time::sleep_until(start + self.lifetime);
        tokio::pin!(expiry);
        let mut bucket = Bucket::new(start);
        let mut posted = 0;
        // Since when due batches have had to wait for the bucket, and until
        // when the one pending now must wait.
        let mut throttled: Option<Instant> = None;
        let mut hold: Option<Instant> = None;
        loop {
            let (due, until, quiet) = {
                let s = self.scan.lock().unwrap();
                let quiet = self
                    .silence
                    .filter(|_| !s.hushed)
                    .map(|(_, d)| s.heard.unwrap_or(start) + d);
                (s.batch.due(self.timing), s.until.is_some(), quiet)
            };
            if until {
                return Err(self.ending(Ending::Until, posted, cx));
            }
            let at = due.map(|d| hold.map_or(d, |h| d.max(h)));
            // Disabled when nothing is pending (the deadline is never polled).
            let sleep = tokio::time::sleep_until(at.unwrap_or(start));
            let silent = tokio::time::sleep_until(quiet.unwrap_or(start));
            tokio::select! {
                biased;
                out = &mut exec => {
                    // Not `match self.scan.lock()…`: the guard would live
                    // through the arms, and `ending` takes the lock again.
                    let until = self.scan.lock().unwrap().until.is_some();
                    return match until {
                        true => Err(self.ending(Ending::Until, posted, cx)),
                        false => Ok(out),
                    };
                }
                _ = &mut expiry => return Err(self.ending(Ending::Expired, posted, cx)),
                _ = self.notify.notified() => {}
                // At most one note per silence, and only after an event or
                // the start, so it stays within the rate of the events.
                _ = silent, if quiet.is_some() => {
                    self.scan.lock().unwrap().hushed = true;
                    self.post_silence(cx);
                    posted += 1;
                }
                _ = sleep, if at.is_some() => {
                    let now = Instant::now();
                    match bucket.take(now, self.timing.refill) {
                        Ok(()) => {
                            // A batch posted when due ends a throttled spell.
                            if hold.take().is_none() {
                                throttled = None;
                            }
                            if self.post(cx) {
                                posted += 1;
                            }
                        }
                        Err(next) => {
                            let since = *throttled.get_or_insert(now);
                            if now - since >= self.timing.flood {
                                return Err(self.ending(Ending::Flooded, posted, cx));
                            }
                            hold = Some(next);
                        }
                    }
                }
            }
        }
    }

    /// Take the pending batch, numbered in the task view. `finished`: the
    /// command exited, so an unfinished last line is scanned as a line;
    /// otherwise (without `match`) its new text joins the batch as is.
    fn take(&self, cx: &Cx, finished: bool) -> Option<Rendered> {
        let mut scan = self.scan.lock().unwrap();
        let s = &mut *scan;
        let now = Instant::now();
        if s.until.is_none() && !s.partial.is_empty() {
            if finished {
                s.seen += 1;
                s.open = false;
                let line = std::mem::take(&mut s.partial);
                self.scan_line(s, s.seen, &line, now);
            } else if self.filter.is_none() && s.partial.len() > s.shown {
                let text = s.partial[s.shown..].to_owned();
                s.shown = s.partial.len();
                s.batch.push(s.seen + 1, &text, now);
            }
        }
        let batch = std::mem::take(&mut s.batch);
        if batch.count == 0 {
            return None;
        }
        // The output so far ends the task's output: count back from its last
        // line. (Later trimming of the buffer shifts older events' numbers.)
        let lines = s.seen + usize::from(s.open);
        let view = match cx.history().task(&self.task) {
            Ok(Some((_, output))) => output.lines().count(),
            _ => lines,
        };
        let at = |n: usize| (n + view).saturating_sub(lines).max(1);
        drop(scan);
        let numbered = self.filter.is_some();
        let show = |(n, text): &(usize, String)| {
            if numbered {
                format!("L{}: {text}", at(*n))
            } else {
                text.clone()
            }
        };
        let mut body: Vec<String> = batch.head.iter().map(show).collect();
        let omitted = batch.count - batch.head.len() - batch.tail.len();
        if omitted > 0 {
            body.push(format!("… [{omitted} lines omitted] …"));
        }
        body.extend(batch.tail.iter().map(show));
        let body = body.join("\n");
        let (from, to) = match (batch.head.first(), batch.tail.back().or(batch.head.last())) {
            (Some(first), Some(last)) => (at(first.0), at(last.0)),
            _ => return None,
        };
        let mut truncated = omitted > 0;
        let mut body = if body.chars().count() > INLINE_LIMIT {
            truncated = true;
            truncate_middle(body, INLINE_LIMIT)
        } else {
            body
        };
        if truncated {
            body.push_str(&format!(
                "\n(whole batch: read {RUMU_WORKSPACE}:/tasks/{}/output offset={from} limit={})",
                self.task,
                to + 1 - from
            ));
        }
        Some(Rendered {
            body,
            count: batch.count,
            from,
            to,
            truncated,
        })
    }

    /// Post the pending batch as a note; `false` when there was none.
    fn post(&self, cx: &Cx) -> bool {
        let Some(r) = self.take(cx, false) else {
            return false;
        };
        let what = match self.filter {
            Some(_) => "matching lines",
            None => "lines",
        };
        let note = Content::note_with(
            format!(
                "watch {}{}: +{} {what} (L {}–{})\n{}",
                self.task, self.label, r.count, r.from, r.to, r.body
            ),
            &WatchEvent {
                task: self.task.clone(),
                from: r.from,
                to: r.to,
                truncated: r.truncated,
                count: r.count,
                matching: self.filter.is_some(),
            },
        );
        let post = Post {
            effects: vec![note],
            wake: true,
            paced: true,
        };
        if let Err(e) = self.inbox.post(&cx.branch, post) {
            warn!(task = %self.task, error = %e, "watch event post failed");
        }
        true
    }

    /// Post that no event came for `silence`.
    fn post_silence(&self, cx: &Cx) {
        let what = match self.filter {
            Some(_) => "no matching line",
            None => "no new output",
        };
        let secs = self.silence.map_or(0, |(s, _)| s);
        let note = Content::note_with(
            format!("watch {}{}: {what} for {secs}s", self.task, self.label),
            &WatchSilence {
                task: self.task.clone(),
                secs,
                matching: self.filter.is_some(),
            },
        );
        let post = Post {
            effects: vec![note],
            wake: true,
            paced: true,
        };
        if let Err(e) = self.inbox.post(&cx.branch, post) {
            warn!(task = %self.task, error = %e, "watch silence post failed");
        }
    }

    /// What the command left pending at its exit: the result after its exit
    /// code.
    pub(super) fn tail(&self, cx: &Cx) -> String {
        self.take(cx, true).map(|r| r.body).unwrap_or_default()
    }

    /// The result text of a watch that ended before its command did.
    fn ending(&self, ending: Ending, posted: usize, cx: &Cx) -> String {
        let pending = self.take(cx, false);
        let mut text = match ending {
            Ending::Until => format!(
                "watch ended: `until` matched the last line below, and the command was killed \
                 ({posted} events before it)"
            ),
            Ending::Expired => {
                let mut text = format!(
                    "watch expired after {}s, and the command was killed; {posted} events \
                     delivered. Start it again if you still need it.",
                    self.lifetime.as_secs()
                );
                if posted == 0 && pending.is_none() {
                    text.push_str(match self.filter {
                        Some(_) => " No line matched: if that is unexpected, loosen `match`.",
                        None => " It printed nothing: if that is unexpected, check the command.",
                    });
                }
                text
            }
            Ending::Flooded => format!(
                "watch stopped: its events kept outrunning the rate limit for {}s, and the \
                 command was killed ({posted} events delivered). Restart it with a stricter \
                 `match`.",
                self.timing.flood.as_secs()
            ),
        };
        if let Some(r) = pending {
            text.push('\n');
            text.push_str(&r.body);
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FakeInbox, FakeRuntime};
    use rumu_core::{
        Branch, BranchId, BranchStatus, Call, NodeId, Plugin, Progress, ProgressSink, SessionId,
        SessionInfo, TaskInfo, TaskKind, TaskStatus, Workspace,
    };
    use serde_json::{Value, json};

    const MS: Duration = Duration::from_millis(1);

    fn timing() -> WatchTiming {
        WatchTiming {
            quiet: 150 * MS,
            max: 400 * MS,
            refill: 2_000 * MS,
            flood: 30_000 * MS,
            second: 100 * MS,
        }
    }

    #[test]
    fn batch_is_due_after_a_quiet_gap_or_the_longest_wait() {
        let t = Instant::now();
        let mut batch = Batch::default();
        assert_eq!(batch.due(timing()), None);
        batch.push(1, "a", t);
        assert_eq!(batch.due(timing()), Some(t + 150 * MS));
        batch.push(2, "b", t + 100 * MS);
        assert_eq!(batch.due(timing()), Some(t + 250 * MS));
        // Steady output: the longest wait caps it.
        batch.push(3, "c", t + 300 * MS);
        assert_eq!(batch.due(timing()), Some(t + 400 * MS));
    }

    #[test]
    fn bucket_allows_a_burst_then_one_post_per_refill() {
        let t = Instant::now();
        let refill = 2_000 * MS;
        let mut bucket = Bucket::new(t);
        for _ in 0..BUCKET {
            assert_eq!(bucket.take(t, refill), Ok(()));
        }
        assert_eq!(bucket.take(t + 500 * MS, refill), Err(t + refill));
        assert_eq!(bucket.take(t + refill, refill), Ok(()));
        assert_eq!(bucket.take(t + refill, refill), Err(t + refill * 2));
        // A long pause refills it up to the burst only.
        for _ in 0..BUCKET {
            assert_eq!(bucket.take(t + refill * 100, refill), Ok(()));
        }
        assert!(bucket.take(t + refill * 100, refill).is_err());
    }

    /// A plugin over `root` with `timing`, a fake inbox, and a background
    /// call's `Cx` whose task (`t-w`) buffers its progress like the kernel's.
    fn setup_with(
        root: &std::path::Path,
        kind: BranchKind,
        timing: WatchTiming,
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
        plugin.watch_timing = timing;
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
            main_branch: branch.clone(),
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
        setup_with(root, kind, timing())
    }

    /// The posted notes' texts; every post is paced and wakes.
    fn texts(inbox: &FakeInbox) -> Vec<String> {
        inbox
            .posts()
            .into_iter()
            .map(|(_, post)| {
                assert!(post.wake && post.paced);
                post.effects[0].text.clone()
            })
            .collect()
    }

    /// Run one watching `bash` call to its end.
    async fn watch(plugin: &WorkspacePlugin, cx: &Cx, args: Value) -> String {
        let out = plugin.call_tool("bash", args, cx).await.unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        out.content.text
    }

    /// Output separated by a quiet gap arrives as separate waking notes whose
    /// line range reads back the same lines from `@rumu:/tasks/<id>`; what is
    /// pending at exit is the result, not a note.
    #[tokio::test]
    async fn watch_posts_batches_and_leaves_the_tail_to_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "echo one; echo two; sleep 0.6; echo three; sleep 0.6; echo tail";
        let out = watch(&plugin, &cx, json!({ "command": script, "watch": {} })).await;
        assert_eq!(out, "exit code: 0\ntail");
        assert_eq!(
            texts(&inbox),
            [
                "watch t-w: +2 lines (L 1–2)\none\ntwo",
                "watch t-w: +1 lines (L 3–3)\nthree",
            ]
        );
        // The pointer lines are the task output's lines.
        let read = plugin
            .call_tool(
                "read",
                json!({ "path": "@rumu:/tasks/t-w/output", "offset": 1, "limit": 2 }),
                &cx,
            )
            .await
            .unwrap();
        assert_eq!(read.content.text, "one\ntwo");
    }

    /// Steady output is still flushed at the longest wait, and an unfinished
    /// line shows after a quiet gap without being repeated when it ends.
    #[tokio::test]
    async fn watch_flushes_steady_output_and_unfinished_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "for i in $(seq 30); do echo $i; sleep 0.05; done; printf 'ask: '; sleep 0.4; \
                      echo yes";
        let out = watch(&plugin, &cx, json!({ "command": script, "watch": {} })).await;
        let posts = texts(&inbox);
        assert!(posts.len() >= 3, "{posts:?}");
        assert!(posts.last().unwrap().ends_with("\nask: "), "{posts:?}");
        // Nothing is lost or repeated between the notes and the result.
        let mut seen: Vec<String> = posts
            .iter()
            .flat_map(|p| p.lines().skip(1).map(str::to_owned).collect::<Vec<_>>())
            .collect();
        seen.extend(out.lines().skip(1).map(str::to_owned));
        let mut want: Vec<String> = (1..=30).map(|i| i.to_string()).collect();
        want.extend(["ask: ".into(), "yes".into()]);
        assert_eq!(seen, want);
    }

    /// With `match` only matching lines are events, each numbered by its line in
    /// the task view (the rest is there too); the description labels the notes.
    #[tokio::test]
    async fn watch_match_posts_numbered_matching_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "echo ok 1; echo ERROR a; echo ok 2; echo ok 3; echo ERROR b; sleep 0.4; \
                      echo ok 4; echo ERROR c";
        let args = json!({
            "command": script,
            "description": "errors in app",
            "watch": { "match": "ERROR" },
        });
        let out = watch(&plugin, &cx, args).await;
        assert_eq!(out, "exit code: 0\nL7: ERROR c");
        assert_eq!(
            texts(&inbox),
            ["watch t-w (errors in app): +2 matching lines (L 2–5)\nL2: ERROR a\nL5: ERROR b"]
        );
        let read = plugin
            .call_tool(
                "read",
                json!({ "path": "@rumu:/tasks/t-w/output", "offset": 1, "limit": 5 }),
                &cx,
            )
            .await
            .unwrap();
        assert_eq!(read.content.text, "ok 1\nERROR a\nok 2\nok 3\nERROR b");
    }

    /// `until` ends the watch at its line: the command is killed, the line is
    /// the last event (in the result, once) and nothing after it counts.
    #[tokio::test]
    async fn watch_until_kills_the_command_at_its_line() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "echo start; sleep 0.4; echo working; echo READY; echo after; sleep 30";
        let started = std::time::Instant::now();
        let out = watch(
            &plugin,
            &cx,
            json!({ "command": script, "watch": { "until": "READY" } }),
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            out,
            "watch ended: `until` matched the last line below, and the command was killed (1 \
             events before it)\nworking\nREADY"
        );
        assert_eq!(texts(&inbox), ["watch t-w: +1 lines (L 1–1)\nstart"]);
    }

    /// `until` matching a line the command printed as it exited (the line
    /// and the exit seen in one poll) still ends the watch: this used to
    /// deadlock on the scan lock, so the task could never end or be killed.
    #[tokio::test]
    async fn watch_until_on_the_last_line_ends() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, _inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let args = serde_json::from_value(json!({ "until": "EXIT=" })).unwrap();
        let watch = plugin.watch_for(&cx, args, None, None).unwrap();
        let exec = async { watch.push("EXIT=0\n") };
        let ended = watch.run(exec, &cx).await.unwrap_err();
        assert_eq!(
            ended,
            "watch ended: `until` matched the last line below, and the command was killed (0 \
             events before it)\nEXIT=0"
        );
    }

    /// The lifetime (`timeout`) kills the command and says so; with nothing
    /// delivered it suggests loosening `match`.
    #[tokio::test]
    async fn watch_expires_after_its_lifetime() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let args = json!({
            "command": "echo noise; sleep 30",
            "timeout": 1,
            "watch": { "match": "ERROR" },
        });
        let started = std::time::Instant::now();
        let out = watch(&plugin, &cx, args).await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            out,
            "watch expired after 1s, and the command was killed; 0 events delivered. Start it \
             again if you still need it. No line matched: if that is unexpected, loosen `match`."
        );
        assert!(inbox.posts().is_empty());
    }

    /// Output beyond the rate limit is deferred into later batches, not lost;
    /// a flood that lasts ends the watch.
    #[tokio::test]
    async fn watch_defers_events_over_the_rate_limit_and_stops_a_flood() {
        let dir = tempfile::tempdir().unwrap();
        let fast = WatchTiming {
            quiet: 10 * MS,
            max: 20 * MS,
            refill: 150 * MS,
            flood: 600 * MS,
            second: 100 * MS,
        };
        let (plugin, _host, inbox, cx, _home) = setup_with(dir.path(), BranchKind::Main, fast);
        let script = "i=0; while true; do i=$((i+1)); echo $i; sleep 0.03; done";
        let out = watch(&plugin, &cx, json!({ "command": script, "watch": {} })).await;
        assert!(out.starts_with("watch stopped: "), "{out}");
        assert!(out.contains("stricter `match`"), "{out}");
        let posts = texts(&inbox);
        assert!(posts.len() > BUCKET as usize, "{posts:?}");
        // Every line up to the end arrived, in order, exactly once.
        let mut seen: Vec<u64> = posts
            .iter()
            .flat_map(|p| {
                p.lines()
                    .skip(1)
                    .map(|l| l.parse().unwrap())
                    .collect::<Vec<_>>()
            })
            .collect();
        seen.extend(out.lines().skip(1).map(|l| l.parse::<u64>().unwrap()));
        let want: Vec<u64> = (1..=seen.len() as u64).collect();
        assert_eq!(seen, want);
    }

    /// A long batch keeps both ends inline and points at all of its lines.
    #[tokio::test]
    async fn watch_long_batches_keep_both_ends_and_point_at_their_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let script = "for i in $(seq 500); do echo line $i; done; sleep 0.6";
        let out = watch(&plugin, &cx, json!({ "command": script, "watch": {} })).await;
        assert_eq!(out, "exit code: 0\n");
        let posts = texts(&inbox);
        assert_eq!(posts.len(), 1, "{posts:?}");
        let note = &posts[0];
        assert!(
            note.starts_with("watch t-w: +500 lines (L 1–500)\nline 1\n"),
            "{note}"
        );
        assert!(note.contains("lines omitted"), "{note}");
        assert!(
            note.ends_with(
                "line 500\n(whole batch: read @rumu:/tasks/t-w/output offset=1 limit=500)"
            ),
            "{note}"
        );
        assert!(note.chars().count() < INLINE_LIMIT + 200);
        let data: WatchEvent = inbox.posts()[0].1.effects[0].decode().unwrap();
        assert!(data.truncated);
    }

    /// `silence` posts once per stretch without events, counting only
    /// matching lines with `match`.
    #[tokio::test]
    async fn watch_silence_posts_once_per_quiet_stretch() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let args = json!({
            "command": "echo hi; sleep 0.8; echo ERROR x; sleep 30",
            "timeout": 2,
            "watch": { "match": "ERROR", "silence": 3 },
        });
        let out = watch(&plugin, &cx, args).await;
        assert!(out.starts_with("watch expired after 2s"), "{out}");
        assert_eq!(
            texts(&inbox),
            [
                "watch t-w: no matching line for 3s",
                "watch t-w: +1 matching lines (L 2–2)\nL2: ERROR x",
                "watch t-w: no matching line for 3s",
            ]
        );
    }

    /// Once the run is dropped (`task.kill`), nothing more is posted.
    #[tokio::test]
    async fn watch_posts_nothing_after_the_task_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), BranchKind::Main);
        let args = json!({ "command": "while true; do echo tick; sleep 0.05; done", "watch": {} });
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

    /// `watch` is refused in a sub-agent, in the foreground and with a bad
    /// pattern.
    #[tokio::test]
    async fn watch_is_refused_in_sub_agents_in_the_foreground_and_with_bad_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let agent = BranchKind::Agent {
            task: "t".into(),
            parent_branch: BranchId::new(),
            call_id: "c".into(),
            via_plugin: false,
            inherit: Default::default(),
        };
        let (plugin, _host, inbox, cx, _home) = setup(dir.path(), agent);
        let args = json!({ "command": "echo hi", "watch": {} });
        let out = plugin.call_tool("bash", args.clone(), &cx).await.unwrap();
        assert!(out.is_error);
        assert!(
            out.content.text.contains("sub-agent"),
            "{}",
            out.content.text
        );

        let (plugin, host, _, cx, _home2) = setup(dir.path(), BranchKind::Main);
        let out = plugin.call_tool("bash", args, &host.cx()).await.unwrap();
        assert!(out.is_error);
        assert!(
            out.content.text.contains("needs `background: true`"),
            "{}",
            out.content.text
        );
        for (extra, want) in [
            (json!({ "watch": { "until": "(" } }), "`watch.until`"),
            (
                json!({ "watch": { "silence": 0 } }),
                "`watch.silence` must be at least 1",
            ),
            (json!({ "watch": { "bogus": 5 } }), "unknown field"),
        ] {
            let mut bad = json!({ "command": "echo hi" });
            bad.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let out = plugin.call_tool("bash", bad, &cx).await.unwrap();
            assert!(out.is_error);
            assert!(out.content.text.contains(want), "{}", out.content.text);
        }
        assert!(inbox.posts().is_empty());
    }
}
