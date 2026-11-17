//! The structured patch `write`/`edit` results carry for the UI
//! (`ToolOutput::with_data`): Claude-Code-style hunks — unified-diff line
//! prefixes, 3 lines of context — rendered by the web's FileDiff. The
//! model-facing text is unchanged and no file content is stored: a `create`
//! carries no hunks (the UI renders the call's own `content`), and an update
//! past the caps below carries none either (the UI falls back to the args).
//! A real-workspace patch also names the file it wrote ([`FilePatch::file`]),
//! which is what lets [`super::revert`] undo it later.

use rumu_cards::diff::Hunk;
use rumu_core::{FileRef, Structured, Utf8TypedPathBuf, Workspace};
use serde::{Deserialize, Serialize};

/// Sides larger than this are not diffed (hunks dropped).
const MAX_DIFF_BYTES: usize = 1 << 20;

/// Payload of the `workspace.file_patch` marker (v1) on real-workspace
/// `write`/`edit` results. `hunks` is `None` for a create (the UI renders
/// the call's `content`) and for an update whose diff was dropped: over the
/// size cap, or against a non-UTF-8 previous version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub(crate) struct FilePatch {
    pub kind: PatchKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunks: Option<Vec<Hunk>>,
    /// The file written, on real workspaces (card writes carry none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<FileRef>,
}

impl FilePatch {
    /// The patch, naming the file it was applied to.
    pub(crate) fn at(mut self, ws: &Workspace, abs: &Utf8TypedPathBuf) -> Self {
        self.file = Some(FileRef {
            ws: ws.name.clone(),
            path: abs.to_string(),
        });
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum PatchKind {
    Create,
    Update,
}

impl Structured for FilePatch {
    const TAG: &'static str = rumu_core::FILE_PATCH_TAG;
    const V: u32 = 1;
}

/// The patch of a write to a file that did not exist.
pub(crate) fn create() -> FilePatch {
    FilePatch {
        kind: PatchKind::Create,
        hunks: None,
        file: None,
    }
}

/// An update whose hunks were dropped: a previous version that could not be
/// read, or one past the caps.
pub(crate) fn undiffed() -> FilePatch {
    FilePatch {
        kind: PatchKind::Update,
        hunks: None,
        file: None,
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
        file: None,
    }
}

/// Diff two texts into hunks, or `None` past the size cap.
fn hunks_of(old: &str, new: &str) -> Option<Vec<Hunk>> {
    (old.len() <= MAX_DIFF_BYTES && new.len() <= MAX_DIFF_BYTES)
        .then(|| rumu_cards::diff::hunks(old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_sides_drop_the_hunks() {
        let big = "x\n".repeat(MAX_DIFF_BYTES / 2 + 1);
        assert!(big.len() > MAX_DIFF_BYTES);
        assert!(hunks_of(&big, "small").is_none(), "old past the byte cap");
        assert!(hunks_of("small", &big).is_none(), "new past the byte cap");
    }
}
