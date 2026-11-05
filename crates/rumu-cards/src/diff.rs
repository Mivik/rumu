//! Line diffs (imara-diff, histogram): unified-diff hunks for the UI's file
//! patches and unified-diff text for the model (card read-check refusals and
//! history versions).

use std::ops::Range;

use imara_diff::{Algorithm, Diff, InternedInput};
use serde::{Deserialize, Serialize};

/// Unchanged lines shown around each change.
pub const CONTEXT: u32 = 3;

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

/// Diff two texts by line into hunks with [`CONTEXT`] lines of context.
pub fn hunks(old: &str, new: &str) -> Vec<Hunk> {
    let input = InternedInput::new(old, new);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let line = |tokens: &[imara_diff::Token], i: u32| input.interner[tokens[i as usize]];
    let (old_len, new_len) = (input.before.len() as u32, input.after.len() as u32);

    // Changes closer than two context windows share a hunk.
    let mut groups: Vec<Vec<imara_diff::Hunk>> = Vec::new();
    for h in diff.hunks() {
        match groups.last_mut() {
            Some(g) if h.before.start - g.last().unwrap().before.end <= 2 * CONTEXT => g.push(h),
            _ => groups.push(vec![h]),
        }
    }

    groups
        .into_iter()
        .map(|g| {
            let (first, last) = (&g[0], &g[g.len() - 1]);
            let lead = first.before.start.min(CONTEXT);
            let old_span = first.before.start - lead..(last.before.end + CONTEXT).min(old_len);
            let new_span = first.after.start - lead..(last.after.end + CONTEXT).min(new_len);
            let mut lines = Vec::new();
            let mut push = |sign: char, text: &str| {
                lines.push(format!("{sign}{}", text.strip_suffix('\n').unwrap_or(text)));
                // Only a side's very last line can lack the separator.
                if !text.ends_with('\n') {
                    lines.push("\\ No newline at end of file".to_owned());
                }
            };
            let mut pos = old_span.start;
            for h in &g {
                for i in pos..h.before.start {
                    push(' ', line(&input.before, i));
                }
                for i in h.before.clone() {
                    push('-', line(&input.before, i));
                }
                for i in h.after.clone() {
                    push('+', line(&input.after, i));
                }
                pos = h.before.end;
            }
            for i in pos..old_span.end {
                push(' ', line(&input.before, i));
            }
            Hunk {
                old_start: start(&old_span),
                old_lines: old_span.len() as u32,
                new_start: start(&new_span),
                new_lines: new_span.len() as u32,
                lines,
            }
        })
        .collect()
}

/// A unified diff of `old` → `new` as text (hunk headers and lines), empty
/// when the texts are equal.
pub fn unified(old: &str, new: &str) -> String {
    let mut out = String::new();
    for h in hunks(old, new) {
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            h.old_start, h.old_lines, h.new_start, h.new_lines
        ));
        for l in h.lines {
            out.push_str(&l);
            out.push('\n');
        }
    }
    out
}

/// The 1-based start of a non-empty span, or the line before an empty one
/// (git's `-0,0`).
fn start(span: &Range<u32>) -> u32 {
    span.start + u32::from(!span.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_in_the_middle() {
        let hs = hunks(
            "one\ntwo\nthree\nfour\nfive\nsix\nseven\n",
            "one\ntwo\nTHREE\nfour\nfive\nsix\nseven\n",
        );
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
    fn nearby_changes_share_a_hunk() {
        let old: String = (1..=12).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l3\n", "L3\n").replace("l8\n", "L8\n");
        let hs = hunks(&old, &new);
        assert_eq!(hs.len(), 1, "{hs:?}");
        assert_eq!((hs[0].old_start, hs[0].old_lines), (1, 11));
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
        assert_eq!(
            hs[0].lines,
            [
                " a",
                "-b",
                "\\ No newline at end of file",
                "+b",
                "+c",
                "\\ No newline at end of file"
            ]
        );
        // A newline gained at the end is a change of the last line.
        let hs = hunks("a\nb", "a\nb\n");
        assert_eq!(
            hs[0].lines,
            [" a", "-b", "\\ No newline at end of file", "+b"]
        );
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
        assert_eq!(unified("same\n", "same\n"), "");
    }

    #[test]
    fn unified_text_has_headers_and_context_only() {
        let old: String = (1..=20).map(|i| format!("line{i}\n")).collect();
        let new = old.replace("line10\n", "LINE10\n");
        assert_eq!(
            unified(&old, &new),
            "@@ -7,7 +7,7 @@\n line7\n line8\n line9\n-line10\n+LINE10\n line11\n line12\n line13\n"
        );
    }
}
