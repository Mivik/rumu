//! The workspace plugin: file + shell tools over workspaces. A workspace is
//! one project (a directory of files) — local, or on a remote host reached
//! over ssh (`ssh: [user@]host` in `RUMU_HOME/config.yaml`). Presets are
//! available to every session and are config only, never model-addressable.
//!
//! Every file tool takes one `path`, optionally qualified as
//! `<workspace>:<path>` (rumu-core `addr`); the `@rumu` workspace is a
//! virtual filesystem holding rumu's own data: cards of the decks mounted on
//! this branch (`@rumu:/cards/<mount>/<path>`, mounted with `deck.mount` on
//! `@rumu:/decks/<name>`), their history
//! (`@rumu:/history`), and, read-only, past conversations
//! (`@rumu:/sessions/...`).
//!
//! The workspace root of the file tools is an addressing convention, not a
//! security boundary — absolute paths and `..` are allowed, and `bash` can
//! reach the whole machine anyway. `read_only` means the file tools refuse
//! writes and `bash` is disabled, nothing more.
//!
//! Layout: [`tools`] (the model-facing tools), [`vfs`] (the `@rumu` path
//! table), [`cards`] (`@rumu:/cards`),
//! [`session`] (`@rumu:/sessions`), [`agents`] (AGENTS.md injection),
//! [`backend`] (local + ssh primitives), [`patch`] (the UI diff payload of
//! `write`/`edit`).

mod agents;
pub(crate) mod backend;
mod cards;
mod decks;
mod history;
mod mount;
mod patch;
mod registry;
mod revert;
mod session;
mod task;
mod time;
mod tools;
pub(crate) mod vfs;
mod watch;

#[cfg(test)]
mod tests;

pub use backend::BackendError;
pub use revert::{RevertAction, RevertFile, RevertPlan, revert_files, revert_note, revert_plan};
pub use tools::snapshot_file;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use rumu_core::{
    Cx, Inbox, Manifest, Plugin, SCRATCH_WORKSPACE, ToolOutput, Workspace, Workspaces,
};
use serde_json::Value;
use tracing::warn;

use backend::{Backend, LocalBackend, SshBackend};
use rumu_cards::Decks;

pub struct WorkspacePlugin {
    home: PathBuf,
    /// The deck registry behind `@rumu:/cards`.
    decks: Decks,
    /// The workspace registry and project lists behind `@rumu:/workspaces`
    /// and the projects' `workspaces:` in `@rumu:/decks` (the kernel merges
    /// the same handle into every path state).
    workspaces: Workspaces,
    /// The blob store behind image reads (`<home>/blobs`), opened lazily on
    /// the first one — most plugin instances never read an image.
    blobs: std::sync::OnceLock<rumu_blob::Blobs>,
    /// ControlPath socket directory for ssh multiplexing (lazily created).
    ssh_ctl_dir: PathBuf,
    /// Probed `uname -sm` per ssh host (cached for the process lifetime).
    ssh_uname: Mutex<HashMap<String, Option<String>>>,
    // `label:root` keys of workspaces with neither AGENTS.md nor
    // CLAUDE.md — the ssh probe is not free, so misses are remembered.
    agents_missing: Mutex<std::collections::HashSet<String>>,
    /// The MCP servers behind `@rumu:/mcp` (and `invoke` of their tools).
    mcp: Option<std::sync::Arc<crate::McpHub>>,
    /// The skills behind `@rumu:/skills`.
    skills: Option<std::sync::Arc<crate::Skills>>,
    /// The kernel's channel for `bash` watch events (set by `attach`).
    inbox: std::sync::OnceLock<std::sync::Arc<dyn Inbox>>,
    /// When a watch posts its pending output.
    watch_timing: watch::WatchTiming,
}

impl WorkspacePlugin {
    pub fn new(home: &Path, decks: Decks, workspaces: Workspaces) -> Self {
        Self {
            home: home.to_path_buf(),
            decks,
            workspaces,
            blobs: std::sync::OnceLock::new(),
            ssh_ctl_dir: ssh_ctl_dir(),
            ssh_uname: Mutex::new(HashMap::new()),
            agents_missing: Mutex::new(std::collections::HashSet::new()),
            mcp: None,
            skills: None,
            inbox: std::sync::OnceLock::new(),
            watch_timing: Default::default(),
        }
    }

    /// Serve `@rumu:/mcp` from `hub`.
    pub fn with_mcp(mut self, hub: std::sync::Arc<crate::McpHub>) -> Self {
        self.mcp = Some(hub);
        self
    }

    /// Serve `@rumu:/skills` from `skills`.
    pub fn with_skills(mut self, skills: std::sync::Arc<crate::Skills>) -> Self {
        self.skills = Some(skills);
        self
    }

    /// The skills, or none found anywhere.
    fn skills(&self) -> std::sync::Arc<crate::Skills> {
        self.skills.clone().unwrap_or_else(|| {
            std::sync::Arc::new(crate::Skills::with_dirs(&self.home, Vec::new(), None))
        })
    }

    /// The MCP servers, or an empty hub's answer when there are none.
    fn mcp(&self) -> std::sync::Arc<crate::McpHub> {
        self.mcp
            .clone()
            .unwrap_or_else(|| crate::McpHub::new(&self.home))
    }

    /// The branch assistant's MCP servers, as its deck has them now.
    fn servers(&self, cx: &Cx) -> rumu_core::Result<rumu_core::McpUse> {
        Ok(crate::mcp::servers(&self.decks, &*cx.state()?))
    }

    /// The branch assistant's skill rules, as its deck has them now.
    fn skill_rules(&self, cx: &Cx) -> rumu_core::Result<Vec<String>> {
        Ok(crate::skills::rules(&self.decks, &*cx.state()?))
    }

    /// The deck registry behind `/cards`.
    pub fn decks(&self) -> &Decks {
        &self.decks
    }

    /// The blob store for image reads, opened on first use (creating
    /// `<home>/blobs`). Cheap to clone; every handle sees the same blobs.
    fn blobs(&self) -> rumu_blob::Result<rumu_blob::Blobs> {
        if let Some(blobs) = self.blobs.get() {
            return Ok(blobs.clone());
        }
        let opened = rumu_blob::Blobs::in_home(&self.home)?;
        let _ = self.blobs.set(opened.clone());
        // Another call may have won the race; either handle is equivalent.
        Ok(self.blobs.get().cloned().unwrap_or(opened))
    }

    fn backend_for(&self, ws: &Workspace) -> std::sync::Arc<dyn Backend> {
        backend_for(&self.ssh_ctl_dir, ws)
    }
}

/// ControlPath socket directory for ssh multiplexing (shared by every
/// backend of one process; created lazily by [`usable_ctl_dir`]).
///
/// Private to the current user — `$XDG_RUNTIME_DIR/rumu-ssh` when set, else
/// `<temp>/rumu-ssh-<uid>` — because a shared temp dir would let another local
/// user pre-create the socket directory.
pub(crate) fn ssh_ctl_dir() -> PathBuf {
    #[cfg(unix)]
    {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
            return Path::new(&runtime).join("rumu-ssh");
        }
        if let Some(uid) = current_uid() {
            return std::env::temp_dir().join(format!("rumu-ssh-{uid}"));
        }
    }
    std::env::temp_dir().join("rumu-ssh")
}

/// The current user's uid, read off the owner of `$HOME` (no libc needed).
#[cfg(unix)]
fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let home = std::env::var_os("HOME")?;
    std::fs::metadata(home).ok().map(|meta| meta.uid())
}

/// Create and validate the ControlPath socket directory. `None` means
/// multiplexing is skipped: the directory could not be created, or it already
/// exists and is not private to the current user (owned by someone else, or
/// group/other accessible — a socket there could be planted).
pub(crate) fn usable_ctl_dir(dir: &Path) -> Option<PathBuf> {
    static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let warn_skip = |why: String| -> Option<PathBuf> {
        if WARNED.set(()).is_ok() {
            warn!(dir = %dir.display(), why = %why, "skipping ssh multiplexing");
        }
        None
    };
    #[cfg(unix)]
    let usable = {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        // symlink_metadata: a symlink planted here is not a directory we own.
        match std::fs::symlink_metadata(dir) {
            Ok(meta) => {
                let ours = meta.is_dir()
                    && current_uid().is_some_and(|uid| meta.uid() == uid)
                    && meta.permissions().mode() & 0o077 == 0;
                ours.then(|| dir.to_path_buf())
                    .or_else(|| warn_skip("ssh control dir is not private".into()))
            }
            Err(_) => match std::fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => Some(dir.to_path_buf()),
                Err(err) => warn_skip(format!("cannot create ssh control dir: {err}")),
            },
        }
    };
    #[cfg(not(unix))]
    let usable = match std::fs::create_dir_all(dir) {
        Ok(()) => Some(dir.to_path_buf()),
        Err(err) => warn_skip(format!("cannot create ssh control dir: {err}")),
    };
    usable
}

/// The backend serving a workspace: the local filesystem, or ssh (with
/// multiplexing keyed by host, so workspaces on one host share the
/// connection).
pub(crate) fn backend_for(ctl_dir: &Path, ws: &Workspace) -> std::sync::Arc<dyn Backend> {
    if ws.name == SCRATCH_WORKSPACE {
        // Created on first use; a failure surfaces in the operation itself.
        let _ = std::fs::create_dir_all(ws.root.as_str());
    }
    match &ws.ssh {
        None => std::sync::Arc::new(LocalBackend {
            root: PathBuf::from(ws.root.as_str()),
        }),
        Some(host) => std::sync::Arc::new(SshBackend {
            host: host.clone(),
            root: ws.root.clone(),
            ctl_dir: usable_ctl_dir(ctl_dir),
        }),
    }
}

#[async_trait]
impl Plugin for WorkspacePlugin {
    fn manifest(&self) -> Manifest {
        tools::manifest()
    }

    fn attach(&self, inbox: std::sync::Arc<dyn Inbox>) {
        let _ = self.inbox.set(inbox);
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        tools::call(self, name, args, cx).await
    }
}

#[cfg(test)]
mod ctl_tests {
    use super::usable_ctl_dir;

    #[test]
    fn ctl_dir_is_created_private_and_refuses_loose_dirs() {
        let dir = std::env::temp_dir().join(format!("rumu-ctl-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(usable_ctl_dir(&dir), Some(dir.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(usable_ctl_dir(&dir), None, "group/other accessible");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
