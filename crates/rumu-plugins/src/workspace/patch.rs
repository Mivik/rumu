//! The structured patch `write`/`edit` results carry for the UI
//! (`ToolOutput::with_data`): Claude-Code-style hunks — unified-diff line
//! prefixes, 3 lines of context — rendered by the web's FileDiff. The
//! model-facing text is unchanged and no file content is stored: a `create`
//! carries no hunks (the UI renders the call's own `content`), and an update
//! past the caps below carries none either (the UI falls back to the args).

use std::time::{Duration, Instant};

use rumu_core::Structured;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

/// Unchanged lines shown around each change.
const CONTEXT: usize = 3;
/// Sides larger than this are not diffed (hunks dropped).
const MAX_DIFF_BYTES: usize = 1 << 20;
/// Wall-clock budget for one diff.
const DIFF_BUDGET: Duration = Duration::from_millis(200);

/// Payload of the `workspace.file_patch` marker (v1) on real-workspace
/// `write`/`edit` results. `hunks` is `None` for a create (the UI renders
/// the call's `content`) and for an update whose diff was dropped: over the
/// size cap, past the time budget, or against a non-UTF-8 previous version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct FilePatch {
    pub kind: PatchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunks: Option<Vec<Hunk>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum PatchKind {
    Create,
    Update,
}

/// One hunk, in unified-diff shape. Starts are 1-based (0 for a side with no
/// lines in the hunk, as in `@@ -0,0 +1,3 @@`); `lines` carry the
/// unified-diff prefixes — `' '` context, `'-'` removed, `'+'` added — and a
/// `"\\ No newline at end of file"` note after a line that lacks one. Line
/// text keeps a trailing `\r` (CRLF files stay lossless); only the `\n` is
/// stripped as the line separator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<String>,
}

impl Structured for FilePatch {
    const TAG: &'static str = "workspace.file_patch";
    const V: u32 = 1;
}

/// The patch of a write to a file that did not exist.
pub(crate) fn create() -> FilePatch {
    FilePatch {
        kind: PatchKind::Create,
        hunks: None,
    }
}

/// An update whose hunks were dropped: a previous version that could not be
/// read, or one past the caps.
pub(crate) fn undiffed() -> FilePatch {
    FilePatch {
        kind: PatchKind::Update,
        hunks: None,
    }
}

/// The patch of `old` → `new`, diffed off the async executor.
pub(crate) async fn update(old: String, new: String) -> FilePatch {
    // A failed join still writes the file; the UI just gets no hunks.
    let hunks = tokio::task::spawn_blocking(move || hunks_of(&old, &new))
        .await
        .ok()
        .flatten();
    FilePatch {
        kind: PatchKind::Update,
        hunks,
    }
}

/// Diff two texts into hunks, or `None` past the caps.
fn hunks_of(old: &str, new: &str) -> Option<Vec<Hunk>> {
    if old.len() > MAX_DIFF_BYTES || new.len() > MAX_DIFF_BYTES {
        return None;
    }
    let deadline = Instant::now() + DIFF_BUDGET;
    let diff = TextDiff::configure()
        .deadline(deadline)
        .diff_lines(old, new);
    // `similar` degrades instead of failing on a missed deadline; drop the
    // whole diff when it took longer than the budget.
    (Instant::now() <= deadline).then(|| {
        diff.grouped_ops(CONTEXT)
            .into_iter()
            .map(|ops| hunk(&diff, &ops))
            .collect()
    })
}

/// One context group as a [`Hunk`].
fn hunk<'diff>(diff: &TextDiff<'diff, '_, '_, str>, ops: &[similar::DiffOp]) -> Hunk {
    let old = ops.first().map(|op| op.old_range()).unwrap_or_default();
    let old_end = ops.last().map(|op| op.old_range().end).unwrap_or(0);
    let new = ops.first().map(|op| op.new_range()).unwrap_or_default();
    let new_end = ops.last().map(|op| op.new_range().end).unwrap_or(0);
    let mut lines = Vec::new();
    for op in ops {
        for change in diff.iter_changes(op) {
            let (sign, value) = match change.tag() {
                ChangeTag::Equal => (' ', change.value()),
                ChangeTag::Delete => ('-', change.value()),
                ChangeTag::Insert => ('+', change.value()),
            };
            lines.push(format!("{sign}{}", strip_newline(value)));
            // Only a side's very last line can lack the separator.
            if !value.ends_with('\n') {
                lines.push("\\ No newline at end of file".to_owned());
            }
        }
    }
    Hunk {
        // An empty side starts at the line before the insertion point
        // (git's `-0,0`); a non-empty one at its first line.
        old_start: start(old.start, old_end > old.start),
        old_lines: (old_end - old.start) as u32,
        new_start: start(new.start, new_end > new.start),
        new_lines: (new_end - new.start) as u32,
        lines,
    }
}

/// The 1-based start of a non-empty span, or its 0-based position when empty.
fn start(pos: usize, non_empty: bool) -> u32 {
    (pos + usize::from(non_empty)) as u32
}

/// Drop the line separator only: a `\r` before it stays in the line text.
fn strip_newline(value: &str) -> &str {
    value.strip_suffix('\n').unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(old: &str, new: &str) -> Vec<Hunk> {
        hunks_of(old, new).expect("small texts always diff")
    }

    #[test]
    fn change_in_the_middle() {
        let hs = hunks("one\ntwo\nthree\nfour\nfive\nsix\nseven\n", "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n");
        assert_eq!(hs.len(), 1);
        assert_eq!(
            hs[0].lines,
            [" one", " two", "-three", "+THREE", " four", " five", " six"]
        );
        assert_eq!((hs[0].old_start, hs[0].old_lines), (1, 6));
        assert_eq!((hs[0].new_start, hs[0].new_lines), (1, 6));
    }

    #[test]
    fn distant_changes_split_into_hunks() {
        // 20 lines with a change at line 4 and an appended tail: the 16
        // unchanged lines between them exceed the 2*CONTEXT merge window.
        let old: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let mut new = old.replace("line4\n", "LINE4\n");
        new.push_str("tail\n");
        let hs = hunks(&old, &new);
        assert_eq!(hs.len(), 2, "{hs:?}");
        assert!(hs[0].lines.contains(&"-line4".to_owned()));
        assert_eq!(hs[1].lines.last(), Some(&"+tail".to_owned()));
        // The tail hunk: the last three lines as context, then the addition.
        assert_eq!((hs[1].old_start, hs[1].old_lines), (18, 3));
        assert_eq!((hs[1].new_start, hs[1].new_lines), (18, 4));
    }

    #[test]
    fn insertion_at_the_start() {
        let hs = hunks("b\nc\n", "a\nb\nc\n");
        assert_eq!(
            hs[0].lines,
            ["+a", " b", " c"],
            "the context follows the insertion"
        );
        assert_eq!(hs[0].old_start, 1);
        assert_eq!(hs[0].new_start, 1);
    }

    #[test]
    fn fully_added_file_starts_at_zero() {
        let hs = hunks("", "a\nb\n");
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].lines, ["+a", "+b"]);
        assert_eq!((hs[0].old_start, hs[0].old_lines), (0, 0));
        assert_eq!((hs[0].new_start, hs[0].new_lines), (1, 2));
    }

    #[test]
    fn missing_trailing_newline_is_marked() {
        let hs = hunks("a\nb", "a\nb\nc");
        assert_eq!(hs[0].lines, [" a", "-b", "\\ No newline at end of file", "+b", "+c", "\\ No newline at end of file"]);
        // A newline gained at the end is a change of the last line.
        let hs = hunks("a\nb", "a\nb\n");
        assert_eq!(hs[0].lines, [" a", "-b", "\\ No newline at end of file", "+b"]);
    }

    #[test]
    fn crlf_lines_keep_their_cr() {
        let hs = hunks("a\r\nb\r\n", "a\r\nB\r\n");
        assert_eq!(hs[0].lines, [" a\r", "-b\r", "+B\r"]);
    }

    #[test]
    fn identical_and_empty_texts_have_no_hunks() {
        assert!(hunks("same\n", "same\n").is_empty());
        assert!(hunks("", "").is_empty());
    }

    #[test]
    fn oversized_sides_drop_the_hunks() {
        let big = "x\n".repeat(MAX_DIFF_BYTES / 2 + 1);
        assert!(big.len() > MAX_DIFF_BYTES);
        assert!(hunks_of(&big, "small").is_none(), "old past the byte cap");
        assert!(hunks_of("small", &big).is_none(), "new past the byte cap");
    }
}
