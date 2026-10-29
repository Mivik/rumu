use serde::{Deserialize, Serialize};
use typed_path::{PathType, Utf8TypedPathBuf};

/// The path style of a local workspace: the compile platform's.
#[cfg(windows)]
const PLATFORM_PATH_TYPE: PathType = PathType::Windows;
#[cfg(not(windows))]
const PLATFORM_PATH_TYPE: PathType = PathType::Unix;

/// A workspace: one project (a directory of files) the model can read/write
/// files in and run commands in.
///
/// Workspaces are **user presets** defined in `RUMU_HOME/config.yaml` and
/// available to every session. Files are addressed as `<name>:<path>`
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

/// The workspace presets from `RUMU_HOME/config.yaml`. Every preset is
/// available to every session; `default` is the initial default workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceCatalog {
    /// Presets in file order.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// `default:` from config.yaml, if it names a preset (or `@rumu`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl WorkspaceCatalog {
    pub fn get(&self, name: &str) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.name == name)
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
