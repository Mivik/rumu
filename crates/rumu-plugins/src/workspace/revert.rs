//! Reverting the file changes made since a user message. The `write`/`edit`
//! results on the way carry their patches ([`FilePatch`]), so undoing them is
//! applying those patches backwards — nothing is snapshotted. A file is only
//! put back while what is on disk still holds what those patches wrote (lines
//! elsewhere may have moved); when anything else changed those lines since —
//! the user, `bash`, another session — it is left alone. Files no patch names
//! (written by `bash`, or past the diff caps) are never touched. A revert
//! leaves a [`FilesReverted`] marker; the writes it undid are not undone
//! twice.

use std::collections::HashMap;

use rumu_cards::diff::Hunk;
use rumu_core::{
    FileRef, FilesReverted, Node, NodeBody, PathState, Timestamp, Utf8TypedPathBuf, Workspace,
    normalize_abs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::backend::{Backend, is_not_found};
use super::patch::{FilePatch, PatchKind};
use super::{backend_for, ssh_ctl_dir};

/// What reverting one file does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RevertAction {
    /// Put the content from before the changes back.
    Restore,
    /// The changes created the file: remove it.
    Delete,
    /// Something else changed what the patches wrote since: left alone.
    Changed,
    /// No full patch was recorded (past the diff caps, not text), or the
    /// workspace is gone or read-only: left alone.
    Unsupported,
}

/// One file the changes touched, and what reverting it does.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RevertFile {
    pub ws: String,
    /// Absolute on the workspace's machine.
    pub path: String,
    /// `<workspace>:<path>` as the tools show it.
    pub label: String,
    pub action: RevertAction,
    /// Lines the revert adds and removes.
    pub added: u32,
    pub removed: u32,
    /// Why the file could not be read or written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
}

/// The files the changes since a user message touched, in the order first
/// touched, and the `bash` commands run meanwhile (their effects are not
/// tracked).
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct RevertPlan {
    pub files: Vec<RevertFile>,
    pub commands: Vec<String>,
}

/// One recorded write.
struct Change {
    file: FileRef,
    kind: PatchKind,
    hunks: Option<Vec<Hunk>>,
    /// A create's content (the call's `content` argument).
    content: Option<String>,
    at: Timestamp,
}

/// A planned file, with what applying it needs.
struct Planned {
    file: RevertFile,
    target: Option<(Workspace, Utf8TypedPathBuf)>,
    /// The content to restore.
    before: Option<String>,
}

/// What reverting `nodes` (a branch's nodes since a user message, with its
/// sub-agents') would do, without changing anything.
pub async fn revert_plan(state: &PathState, nodes: &[Node]) -> RevertPlan {
    let (planned, commands) = plan(state, nodes).await;
    RevertPlan {
        files: planned.into_iter().map(|p| p.file).collect(),
        commands,
    }
}

/// Revert the files among `only` whose plan restores or deletes them,
/// planned afresh. Returns that plan and the files reverted, `error` set on
/// the ones that failed.
pub async fn revert_files(
    state: &PathState,
    nodes: &[Node],
    only: &[FileRef],
) -> (RevertPlan, Vec<RevertFile>) {
    let (planned, commands) = plan(state, nodes).await;
    let plan = RevertPlan {
        files: planned.iter().map(|p| p.file.clone()).collect(),
        commands,
    };
    let mut done = Vec::new();
    for p in planned {
        let picked = only
            .iter()
            .any(|f| f.ws == p.file.ws && f.path == p.file.path);
        let Some((ws, abs)) = p.target.filter(|_| picked) else {
            continue;
        };
        let backend = backend_for(&ssh_ctl_dir(), &ws);
        let result = match (p.file.action, p.before) {
            (RevertAction::Restore, Some(before)) => {
                backend.write_bytes(&abs, before.as_bytes()).await
            }
            (RevertAction::Delete, _) => backend.remove_file(&abs).await,
            _ => continue,
        };
        let mut file = p.file;
        file.error = result.err().map(|e| e.to_string());
        done.push(file);
    }
    (plan, done)
}

/// The note telling the model what happened: the files put back since the
/// prompt beginning `prompt`, and those still holding its changes.
pub fn revert_note(prompt: &str, plan: &RevertPlan, done: &[RevertFile]) -> String {
    let snippet: String = prompt.chars().take(80).collect();
    let ellipsis = if snippet.len() < prompt.len() {
        "…"
    } else {
        ""
    };
    let mut out = format!(
        "The user reverted the file changes made since their message \"{snippet}{ellipsis}\":"
    );
    let ok = |a: RevertAction| {
        done.iter()
            .filter(move |f| f.error.is_none() && f.action == a)
            .map(|f| f.label.as_str())
            .collect::<Vec<_>>()
    };
    let (restored, deleted) = (ok(RevertAction::Restore), ok(RevertAction::Delete));
    if !restored.is_empty() {
        out.push_str(&format!("\n- restored: {}", restored.join(", ")));
    }
    if !deleted.is_empty() {
        out.push_str(&format!("\n- deleted: {}", deleted.join(", ")));
    }
    let kept: Vec<&str> = plan
        .files
        .iter()
        .filter(|f| {
            !done
                .iter()
                .any(|d| d.error.is_none() && d.ws == f.ws && d.path == f.path)
        })
        .map(|f| f.label.as_str())
        .collect();
    if !kept.is_empty() {
        out.push_str(&format!("\n- left as they are: {}", kept.join(", ")));
    }
    if !plan.commands.is_empty() {
        out.push_str("\nWhatever the bash commands since then changed was not undone.");
    }
    out
}

/// Plan every file the recorded writes in `nodes` touched.
async fn plan(state: &PathState, nodes: &[Node]) -> (Vec<Planned>, Vec<String>) {
    let (changes, commands) = changes_of(nodes);
    let mut order: Vec<&FileRef> = Vec::new();
    let mut by_file: HashMap<&FileRef, Vec<&Change>> = HashMap::new();
    for c in &changes {
        by_file
            .entry(&c.file)
            .or_insert_with(|| {
                order.push(&c.file);
                Vec::new()
            })
            .push(c);
    }
    let mut planned = Vec::new();
    for file in order {
        if let Some(p) = plan_file(state, file, &by_file[file]).await {
            planned.push(p);
        }
    }
    (planned, commands)
}

/// The recorded writes in `nodes` that no revert among them undid, oldest
/// first, and the `bash` commands.
fn changes_of(nodes: &[Node]) -> (Vec<Change>, Vec<String>) {
    let mut calls: HashMap<&str, (&str, &Value)> = HashMap::new();
    let mut changes = Vec::new();
    let mut commands = Vec::new();
    // (since, at, files) of each revert: the writes to `files` in between
    // are undone.
    let mut reverts: Vec<(Timestamp, Timestamp, Vec<FileRef>)> = Vec::new();
    for n in nodes {
        match &n.body {
            NodeBody::Note { content, .. } => {
                if let Some(m) = content.decode::<FilesReverted>() {
                    // A revert from before `nodes` undid all before it.
                    let since = nodes
                        .iter()
                        .find(|x| x.id == m.from)
                        .map_or(Timestamp::MIN, |x| x.at);
                    reverts.push((since, n.at, m.files));
                }
            }
            NodeBody::Assistant { tool_calls, .. } => {
                for c in tool_calls {
                    calls.insert(&c.call_id, (&c.name, &c.args));
                }
            }
            NodeBody::ToolResult {
                call_id,
                content,
                is_error,
                invoke,
            } => {
                let call = match invoke {
                    Some(i) => Some((i.name.as_str(), &i.args)),
                    None => calls.get(call_id.as_str()).copied(),
                };
                let Some((name, args)) = call else { continue };
                if name == "bash"
                    && let Some(cmd) = args.get("command").and_then(Value::as_str)
                {
                    commands.push(cmd.to_owned());
                }
                if *is_error {
                    continue;
                }
                let Some(patch) = content.decode::<FilePatch>() else {
                    continue;
                };
                let Some(file) = patch.file else { continue };
                let content = (patch.kind == PatchKind::Create)
                    .then(|| {
                        args.get("content")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .flatten();
                changes.push(Change {
                    file,
                    kind: patch.kind,
                    hunks: patch.hunks,
                    content,
                    at: n.at,
                });
            }
            _ => {}
        }
    }
    changes.retain(|c| {
        !reverts
            .iter()
            .any(|(since, at, files)| (*since..*at).contains(&c.at) && files.contains(&c.file))
    });
    changes.sort_by_key(|c| c.at);
    (changes, commands)
}

/// Plan one file from its writes (oldest first); `None` when reverting it
/// changes nothing.
async fn plan_file(state: &PathState, file: &FileRef, changes: &[&Change]) -> Option<Planned> {
    let mut out = RevertFile {
        ws: file.ws.clone(),
        path: file.path.clone(),
        label: format!("{}:{}", file.ws, file.path),
        action: RevertAction::Unsupported,
        added: 0,
        removed: 0,
        error: None,
    };
    let left = |file: RevertFile| {
        Some(Planned {
            file,
            target: None,
            before: None,
        })
    };
    let Some(ws) = state.workspaces.iter().find(|w| w.name == file.ws) else {
        return left(out);
    };
    let Ok(abs) = normalize_abs(ws, &file.path) else {
        return left(out);
    };
    out.label = rumu_core::display(&ws.name, ws.root.to_path(), abs.to_path());
    let complete = changes.iter().all(|c| match c.kind {
        PatchKind::Update => c.hunks.is_some(),
        PatchKind::Create => c.content.is_some(),
    });
    if ws.read_only || !complete {
        return left(out);
    }
    let backend = backend_for(&ssh_ctl_dir(), ws);
    let now = match read_text(&*backend, &abs).await {
        Ok(now) => now,
        Err(e) => {
            out.action = RevertAction::Changed;
            out.error = e;
            return left(out);
        }
    };
    out.action = RevertAction::Changed;
    let Some(before) = unwind(now.clone(), changes) else {
        return left(out);
    };
    let (action, before) = match (&now, before) {
        (Some(now), Some(before)) if *now == before => return None,
        (Some(_), Some(before)) => (RevertAction::Restore, Some(before)),
        (Some(_), None) => (RevertAction::Delete, None),
        // Unwinding needs a file to start from.
        (None, _) => return left(out),
    };
    let (added, removed) = line_counts(
        now.as_deref().unwrap_or(""),
        before.as_deref().unwrap_or(""),
    );
    out.action = action;
    out.added = added;
    out.removed = removed;
    Some(Planned {
        file: out,
        target: Some((ws.clone(), abs)),
        before,
    })
}

/// A file's text, `None` when it does not exist. `Err(None)` when it is not
/// UTF-8 (so not what a patch wrote), `Err(Some(_))` when it cannot be read.
async fn read_text(
    backend: &dyn Backend,
    abs: &Utf8TypedPathBuf,
) -> Result<Option<String>, Option<String>> {
    match backend.read_bytes(abs).await {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| None),
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(Some(e.to_string())),
    }
}

/// Undo `changes` (oldest first) on `now`, newest first: the content before
/// the oldest (`None`: the file did not exist), or `None` when something
/// else changed what they wrote.
fn unwind(now: Option<String>, changes: &[&Change]) -> Option<Option<String>> {
    let mut cur = now;
    for c in changes.iter().rev() {
        let text = cur?;
        cur = match c.kind {
            PatchKind::Update => Some(unapply(&text, c.hunks.as_deref()?)?),
            PatchKind::Create => {
                if Some(&text) != c.content.as_ref() {
                    return None;
                }
                None
            }
        };
    }
    Some(cur)
}

/// Apply `hunks` (old → new) backwards to `text`, or `None` when a hunk's new
/// side is not found in it. Each hunk is looked for at its recorded line
/// first, then at the nearest place it matches whole.
fn unapply(text: &str, hunks: &[Hunk]) -> Option<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    // Last hunk first: the earlier ones' recorded lines stay put.
    for h in hunks.iter().rev() {
        let (old, new) = sides(h);
        let at = find(&lines, &new, h.new_start.saturating_sub(1) as usize)?;
        lines.splice(at..at + new.len(), old);
    }
    Some(lines.concat())
}

/// A hunk's old and new lines, each with its `\n` unless the
/// "No newline at end of file" note follows it.
fn sides(h: &Hunk) -> (Vec<String>, Vec<String>) {
    let (mut old, mut new) = (Vec::<String>::new(), Vec::<String>::new());
    let mut last = ' ';
    for l in &h.lines {
        let mut chars = l.chars();
        let sign = chars.next().unwrap_or(' ');
        if sign == '\\' {
            let strip = |side: &mut Vec<String>| {
                if let Some(s) = side.last_mut() {
                    s.pop();
                }
            };
            if last != '+' {
                strip(&mut old);
            }
            if last != '-' {
                strip(&mut new);
            }
            continue;
        }
        let line = format!("{}\n", chars.as_str());
        match sign {
            '-' => old.push(line),
            '+' => new.push(line),
            _ => {
                old.push(line.clone());
                new.push(line);
            }
        }
        last = sign;
    }
    (old, new)
}

/// Where `block` sits in `lines`: at `expect` when it matches there, else the
/// nearest match.
fn find(lines: &[String], block: &[String], expect: usize) -> Option<usize> {
    if block.is_empty() {
        // Only an emptied file has a hunk with no new lines.
        return lines.is_empty().then_some(0);
    }
    let fits = |i: usize| lines.get(i..i + block.len()) == Some(block);
    if fits(expect) {
        return Some(expect);
    }
    (0..=lines.len().saturating_sub(block.len()))
        .filter(|&i| fits(i))
        .min_by_key(|&i| i.abs_diff(expect))
}

/// Lines added and removed going from `now` to `before`.
fn line_counts(now: &str, before: &str) -> (u32, u32) {
    let mut counts = (0, 0);
    for h in rumu_cards::diff::hunks(now, before) {
        for l in &h.lines {
            match l.as_bytes().first() {
                Some(b'+') => counts.0 += 1,
                Some(b'-') => counts.1 += 1,
                _ => {}
            }
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_cards::diff::hunks;

    fn undo(old: &str, new: &str, now: &str) -> Option<String> {
        unapply(now, &hunks(old, new))
    }

    #[test]
    fn unapplying_restores_the_old_text() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\nI\nj\nk\n";
        assert_eq!(undo(old, new, new).as_deref(), Some(old));
    }

    #[test]
    fn missing_final_newlines_round_trip() {
        for (old, new) in [
            ("a\nb", "a\nb\n"),
            ("a\nb\n", "a\nb"),
            ("x", "y"),
            ("", "a\n"),
            ("a\n", ""),
        ] {
            assert_eq!(
                undo(old, new, new).as_deref(),
                Some(old),
                "{old:?} -> {new:?}"
            );
        }
    }

    #[test]
    fn hunks_follow_lines_moved_elsewhere() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let new = "1\n2\n3\n4\nfive\n6\n7\n8\n";
        // Someone added two lines at the top since.
        let now = format!("x\ny\n{new}");
        assert_eq!(undo(old, new, &now), Some(format!("x\ny\n{old}")));
    }

    #[test]
    fn a_hunk_whose_lines_changed_since_is_refused() {
        let old = "1\n2\n3\n";
        let new = "1\ntwo\n3\n";
        assert_eq!(undo(old, new, "1\nTWO\n3\n"), None);
    }

    fn change(kind: PatchKind, hunks: Option<Vec<Hunk>>, content: Option<&str>) -> Change {
        Change {
            file: FileRef {
                ws: "w".into(),
                path: "/f".into(),
            },
            kind,
            hunks,
            content: content.map(str::to_owned),
            at: 0,
        }
    }

    #[test]
    fn unwinding_a_create_then_edits_deletes_the_file() {
        let create = change(PatchKind::Create, None, Some("a\n"));
        let edit = change(PatchKind::Update, Some(hunks("a\n", "a\nb\n")), None);
        assert_eq!(unwind(Some("a\nb\n".into()), &[&create, &edit]), Some(None));
        // Edited by hand since: refused.
        assert_eq!(unwind(Some("a\nb\nc\n".into()), &[&create, &edit]), None);
        // Gone already: nothing to unwind from.
        assert_eq!(unwind(None, &[&create, &edit]), None);
    }
}
