//! `RUMU_HOME/config.yaml`: the whole rumu config — the default workspace,
//! the workspace presets (string shorthand or `{root, ssh?, read_only?}`),
//! and the external plugin list — parsed once by [`load_config`]. The
//! workspace catalog and the external plugin specs are derived from it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rumu_core::{IoError, RUMU_WORKSPACE, Workspace, WorkspaceCatalog, valid_workspace_name};
use serde::de;
use serde::{Deserialize, Deserializer};
use tracing::warn;

use crate::{Error, Result};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    workspaces: BTreeMap<String, WorkspaceEntry>,
    #[serde(default)]
    plugins: Vec<PluginEntry>,
    /// Whether a config file was actually there (a missing one means
    /// [`Config::default`], and the cwd becomes the workspace).
    #[serde(skip)]
    present: bool,
}

/// One workspace entry: the string shorthand (`proj: ~/code/proj`) or the
/// map form (`{ root, ssh?, read_only? }`).
#[derive(Debug)]
enum WorkspaceEntry {
    Root(String),
    Detailed {
        root: String,
        ssh: Option<String>,
        read_only: bool,
    },
}

impl WorkspaceEntry {
    fn parts(&self) -> (&str, Option<&str>, bool) {
        match self {
            Self::Root(root) => (root, None, false),
            Self::Detailed {
                root,
                ssh,
                read_only,
            } => (root, ssh.as_deref(), *read_only),
        }
    }
}

impl<'de> Deserialize<'de> for WorkspaceEntry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::String(root) => Ok(Self::Root(root)),
            serde_json::Value::Object(map) => {
                let mut root = None;
                let mut ssh = None;
                let mut read_only = false;
                for (key, val) in map {
                    match key.as_str() {
                        "root" => {
                            root = Some(
                                val.as_str()
                                    .ok_or_else(|| de::Error::custom("`root` expects a string"))?
                                    .to_owned(),
                            )
                        }
                        "ssh" => {
                            // `ssh: null` (YAML) is the same as omitting it.
                            if !val.is_null() {
                                ssh = Some(
                                    val.as_str()
                                        .ok_or_else(|| de::Error::custom("`ssh` expects a string"))?
                                        .to_owned(),
                                );
                            }
                        }
                        "read_only" => {
                            read_only = val
                                .as_bool()
                                .ok_or_else(|| de::Error::custom("`read_only` expects a boolean"))?
                        }
                        other => {
                            return Err(de::Error::custom(format!("unknown field `{other}`")));
                        }
                    }
                }
                let root = root.ok_or_else(|| de::Error::custom("workspace is missing `root`"))?;
                Ok(Self::Detailed {
                    root,
                    ssh,
                    read_only,
                })
            }
            _ => Err(de::Error::custom(
                "a workspace is a root path string or a `{root, ssh?, read_only?}` map",
            )),
        }
    }
}

/// One external plugin entry in `plugins:`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginEntry {
    name: Option<String>,
    #[serde(default)]
    cmd: Vec<String>,
    cwd: Option<String>,
}

/// A resolved external plugin entry.
pub(crate) struct ExternalPluginSpec {
    pub name: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

/// Read and parse `<home>/config.yaml`. A missing file is an empty
/// [`Config`]; invalid YAML or an unknown key is a load error.
pub fn load_config(home: &Path) -> Result<Config> {
    let path = home.join("config.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(source) => return Err(IoError::at("reading", &path)(source).into()),
    };
    let mut config: Config = rumu_cards::yaml::from_str(&text).map_err(|source| Error::Yaml {
        path,
        source: Box::new(source),
    })?;
    config.present = true;
    Ok(config)
}

impl Config {
    /// The workspace presets. Invalid names, bad ssh targets, non-absolute
    /// ssh roots and unknown keys are errors — a workspace that silently
    /// vanishes is worse than a startup failure. `~` in local roots expands
    /// to `$HOME`.
    pub fn catalog(&self) -> Result<WorkspaceCatalog> {
        let mut workspaces = Vec::new();
        for (name, entry) in &self.workspaces {
            validate_explicit_name(name)?;
            let (root, ssh, read_only) = entry.parts();
            push_workspace(&mut workspaces, name, root, ssh, read_only)?;
        }
        let default = self
            .default
            .as_deref()
            .filter(|d| {
                let known = *d == RUMU_WORKSPACE || workspaces.iter().any(|w| w.name == *d);
                if !known {
                    warn!(default = %d, "`default` does not name a preset; ignoring");
                }
                known
            })
            .map(str::to_owned);
        Ok(WorkspaceCatalog {
            workspaces,
            default,
        })
    }

    /// [`Config::catalog`], except that with no config file at all the current
    /// working directory becomes a local read-write workspace (zero-config
    /// startup). Its name is the directory's basename, lowercased with other
    /// characters folded to `-`; an unusable basename becomes `ws`.
    pub fn catalog_or_cwd(&self) -> Result<WorkspaceCatalog> {
        if self.present {
            return self.catalog();
        }
        let cwd = std::env::current_dir()
            .map_err(|source| Error::Io(IoError::op("resolving the current directory", source)))?;
        let root = cwd.to_string_lossy().into_owned();
        let name = cwd_workspace_name(&root);
        Ok(WorkspaceCatalog {
            workspaces: vec![Workspace::new(name.clone(), None, root, false)],
            default: Some(name),
        })
    }

    /// The external plugin list (`plugins:`), resolved against `home`.
    pub(crate) fn external_specs(&self, home: &Path) -> Result<Vec<ExternalPluginSpec>> {
        let path = home.join("config.yaml");
        let mut specs = Vec::new();
        for ext in &self.plugins {
            if ext.cmd.is_empty() {
                return Err(Error::Config {
                    path: path.clone(),
                    message: "external plugin entry has an empty `cmd`".into(),
                });
            }
            let name = ext.name.clone().unwrap_or_else(|| {
                Path::new(&ext.cmd[0])
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ext.cmd[0].clone())
            });
            let cwd = home.join(ext.cwd.as_deref().unwrap_or("."));
            specs.push(ExternalPluginSpec {
                name,
                argv: ext.cmd.clone(),
                cwd,
            });
        }
        Ok(specs)
    }
}

fn cwd_workspace_name(root: &str) -> String {
    let base = Path::new(root)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name: String = base
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if valid_workspace_name(&name) {
        name
    } else {
        "ws".to_owned()
    }
}

fn push_workspace(
    out: &mut Vec<Workspace>,
    name: &str,
    root: &str,
    ssh: Option<&str>,
    read_only: bool,
) -> Result<()> {
    if root.is_empty() {
        return Err(Error::WorkspaceConfig {
            name: name.to_owned(),
            message: "workspace root must be non-empty".into(),
        });
    }
    let Some(ssh) = ssh else {
        // `~` means $HOME — only on the machine rumu runs on.
        out.push(Workspace::new(
            name.to_owned(),
            None,
            expand_tilde(root),
            read_only,
        ));
        return Ok(());
    };
    if ssh.is_empty() || ssh.chars().any(char::is_whitespace) {
        return Err(Error::WorkspaceConfig {
            name: name.to_owned(),
            message: format!("ssh host must be non-empty and contain no whitespace (got `{ssh}`)"),
        });
    }
    // An ssh root must be absolute: there is no local cwd to resolve
    // against, and a silent miss is worse than a load error.
    if !root.starts_with('/') {
        return Err(Error::WorkspaceConfig {
            name: name.to_owned(),
            message: format!("ssh `root` must be an absolute path (got `{root}`)"),
        });
    }
    out.push(Workspace::new(
        name.to_owned(),
        Some(ssh.to_owned()),
        root.to_owned(),
        read_only,
    ));
    Ok(())
}

fn validate_explicit_name(name: &str) -> Result<()> {
    if valid_workspace_name(name) {
        return Ok(());
    }
    Err(Error::WorkspaceConfig {
        name: name.to_owned(),
        message: format!(
            "invalid workspace name (rule: `[a-z][a-z0-9.-]*`, length >= 2; the common URI \
             schemes are reserved; `{RUMU_WORKSPACE}` is reserved for rumu's own data)"
        ),
    })
}

fn expand_tilde(s: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if s == "~" {
        return home;
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixture_home;

    #[test]
    fn external_specs_from_config_yaml() {
        let home = fixture_home();
        let config = load_config(&home).unwrap();
        let specs = config.external_specs(&home).unwrap();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["clock", "compact"]);
        assert_eq!(specs[0].argv, ["python3", "plugins/clock.py"]);
        assert_eq!(specs[0].cwd, home);

        // No config at all is an empty (not failed) load.
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(config.external_specs(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn missing_config_is_the_default_and_unknown_keys_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(config.catalog().unwrap().workspaces.is_empty());

        std::fs::write(dir.path().join("config.yaml"), "machines: {}\n").unwrap();
        let err = load_config(dir.path()).unwrap_err().to_string();
        assert!(err.contains("unknown field `machines`"), "{err}");
    }
}
