use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use typed_path::{PathType, Utf8TypedPathBuf};

use crate::{SCRATCH_WORKSPACE, SessionId};

/// The path style of a local workspace: the compile platform's.
#[cfg(windows)]
const PLATFORM_PATH_TYPE: PathType = PathType::Windows;
#[cfg(not(windows))]
const PLATFORM_PATH_TYPE: PathType = PathType::Unix;

/// A workspace: one project (a directory of files) the model can read/write
/// files in and run commands in.
///
/// Workspaces are registered in [`Workspaces`] and mounted by projects.
/// Files are addressed as `<name>:<path>`
/// (see [`crate::addr`]); the implicit `@rumu` workspace holds rumu's own
/// data and is never a preset (its `@` is not a valid preset-name character).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(from = "RawWorkspace", into = "RawWorkspace")]
#[ts(export)]
pub struct Workspace {
    pub name: String,
    /// `[user@]host` when this workspace lives on a remote ssh host; `None`
    /// for local.
    #[ts(optional)]
    pub ssh: Option<String>,
    /// The workspace root: an absolute directory path on its machine, in the
    /// workspace's [path style](Workspace::path_type). Tool `path`s are
    /// relative to it (absolute paths on the machine are also allowed).
    #[ts(type = "string")]
    pub root: Utf8TypedPathBuf,
    /// Read-only workspaces refuse writes and disable `bash`.
    pub read_only: bool,
}

/// The wire form of [`Workspace`]: `root` is a plain string and its path
/// style comes from the `ssh` field.
#[derive(Serialize, Deserialize)]
struct RawWorkspace {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ssh: Option<String>,
    root: String,
    #[serde(default)]
    read_only: bool,
}

impl From<RawWorkspace> for Workspace {
    fn from(raw: RawWorkspace) -> Self {
        Self::new(raw.name, raw.ssh, raw.root, raw.read_only)
    }
}

impl From<Workspace> for RawWorkspace {
    fn from(ws: Workspace) -> Self {
        Self {
            name: ws.name,
            ssh: ws.ssh,
            root: ws.root.as_str().to_owned(),
            read_only: ws.read_only,
        }
    }
}

impl Workspace {
    /// Build a workspace, parsing `root` in the workspace's path style: ssh
    /// workspaces are POSIX (Unix), local ones use the compile platform's.
    pub fn new(name: String, ssh: Option<String>, root: String, read_only: bool) -> Self {
        let path_type = if ssh.is_some() {
            PathType::Unix
        } else {
            PLATFORM_PATH_TYPE
        };
        let root = match path_type {
            PathType::Unix => Utf8TypedPathBuf::from_unix(root),
            PathType::Windows => Utf8TypedPathBuf::from_windows(root),
        };
        Self {
            name,
            ssh,
            root,
            read_only,
        }
    }

    /// The path style of this workspace: ssh targets are POSIX, local roots
    /// carry the compile platform's style (unless a test built one directly
    /// with another style).
    pub fn path_type(&self) -> PathType {
        if self.ssh.is_some() {
            PathType::Unix
        } else if self.root.is_windows() {
            PathType::Windows
        } else {
            PathType::Unix
        }
    }

    /// Is this workspace on the machine rumu runs on?
    pub fn is_local(&self) -> bool {
        self.ssh.is_none()
    }

    /// The compact listing label: `local`, or `ssh:<host>`.
    pub fn label(&self) -> String {
        match &self.ssh {
            None => "local".to_owned(),
            Some(host) => format!("ssh:{host}"),
        }
    }
}

/// The mount point of a session's project deck: the deck mounted there is
/// the session's project.
pub const PROJECT_MOUNT: &str = "/cards/project";

/// Each project's mounted workspaces: project deck id → workspace names, in
/// order (the first is the project's default).
pub type ProjectWorkspaces = BTreeMap<String, Vec<String>>;

/// The workspace registry: every workspace the user added
/// (`RUMU_HOME/workspaces.yaml`) and the workspaces each project mounts
/// (`RUMU_HOME/projects.yaml`), both written only by rumu. Every registered
/// workspace is addressable from every session; a session's project decides
/// which are mounted (see [`crate::PathState`]). Cheap to clone; every
/// handle sees the same lists, so edits apply to running sessions.
///
/// It also knows where sessions' scratch directories live (see
/// [`crate::SCRATCH_WORKSPACE`]): `RUMU_HOME/scratch` once loaded, a
/// directory under the system temp dir until then.
#[derive(Clone)]
pub struct Workspaces {
    inner: Arc<RwLock<Registry>>,
    listeners: Arc<RwLock<Vec<Listener>>>,
    scratch: PathBuf,
}

impl Default for Workspaces {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

type Listener = Box<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Default)]
struct Registry {
    list: Vec<Workspace>,
    projects: ProjectWorkspaces,
}

impl std::fmt::Debug for Workspaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.read().unwrap().fmt(f)
    }
}

impl Workspaces {
    pub fn new(list: Vec<Workspace>) -> Self {
        Self::with_projects(list, ProjectWorkspaces::new())
    }

    pub fn with_projects(mut list: Vec<Workspace>, projects: ProjectWorkspaces) -> Self {
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Self {
            inner: Arc::new(RwLock::new(Registry { list, projects })),
            listeners: Arc::default(),
            scratch: std::env::temp_dir().join("rumu-scratch"),
        }
    }

    /// Keep sessions' scratch directories under `dir`.
    pub fn with_scratch(mut self, dir: PathBuf) -> Self {
        self.scratch = dir;
        self
    }

    /// The scratch workspace of `session`. Nothing is created here: the
    /// directory appears on first use.
    pub fn scratch(&self, session: &SessionId) -> Workspace {
        let root = self.scratch.join(session.to_string());
        Workspace::new(
            SCRATCH_WORKSPACE.to_owned(),
            None,
            root.to_string_lossy().into_owned(),
            false,
        )
    }

    /// The registered workspaces, in name order.
    pub fn list(&self) -> Vec<Workspace> {
        self.inner.read().unwrap().list.clone()
    }

    pub fn get(&self, name: &str) -> Option<Workspace> {
        self.inner
            .read()
            .unwrap()
            .list
            .iter()
            .find(|w| w.name == name)
            .cloned()
    }

    /// Every project's mounted workspaces, as stored (names that left the
    /// registry included).
    pub fn projects(&self) -> ProjectWorkspaces {
        self.inner.read().unwrap().projects.clone()
    }

    /// The registered workspaces `project` mounts, in its order.
    pub fn project(&self, project: &str) -> Vec<String> {
        let inner = self.inner.read().unwrap();
        inner
            .projects
            .get(project)
            .into_iter()
            .flatten()
            .filter(|n| inner.list.iter().any(|w| &w.name == *n))
            .cloned()
            .collect()
    }

    /// Call `f` after every change to either list.
    pub fn on_change(&self, f: impl Fn() + Send + Sync + 'static) {
        self.listeners.write().unwrap().push(Box::new(f));
    }

    /// Replace the workspace list through `change`, under the write lock;
    /// nothing changes when it fails (so a failed save leaves the old list).
    pub fn update<E>(
        &self,
        change: impl FnOnce(&mut Vec<Workspace>) -> Result<(), E>,
    ) -> Result<(), E> {
        self.update_all(|reg| {
            change(&mut reg.list)?;
            reg.list.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(())
        })
    }

    /// Replace both lists through `change`, as [`update`](Self::update).
    pub fn update_both<E>(
        &self,
        change: impl FnOnce(&mut Vec<Workspace>, &mut ProjectWorkspaces) -> Result<(), E>,
    ) -> Result<(), E> {
        self.update_all(|reg| {
            change(&mut reg.list, &mut reg.projects)?;
            reg.list.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(())
        })
    }

    fn update_all<E>(&self, change: impl FnOnce(&mut Registry) -> Result<(), E>) -> Result<(), E> {
        {
            let mut guard = self.inner.write().unwrap();
            let mut next = guard.clone();
            change(&mut next)?;
            *guard = next;
        }
        for listener in self.listeners.read().unwrap().iter() {
            listener();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_style_follows_ssh_and_the_wire_form_is_a_plain_string() {
        let ssh = Workspace::new(
            "app".into(),
            Some("deploy@prod".into()),
            "/srv/app".into(),
            true,
        );
        assert!(ssh.root.is_unix(), "ssh roots are POSIX");

        let json = serde_json::to_value(&ssh).unwrap();
        assert_eq!(json["root"], "/srv/app");
        assert_eq!(serde_json::from_value::<Workspace>(json).unwrap(), ssh);

        // A local root is parsed in the compile platform's style.
        let local = Workspace::new("proj".into(), None, r"C:\Users\me\proj".into(), false);
        #[cfg(windows)]
        assert!(local.root.is_windows());
        #[cfg(not(windows))]
        assert!(local.root.is_unix());
    }
}
