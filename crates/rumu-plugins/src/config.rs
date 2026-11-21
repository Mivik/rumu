//! `RUMU_HOME/config.yaml`: the user's hand-written config — the external
//! plugin list — parsed once by [`load_config`]. Workspaces live in
//! `RUMU_HOME/workspaces.yaml` (see [`crate::workspaces`]), decks in
//! `RUMU_HOME/decks/`.

use std::path::{Path, PathBuf};

use rumu_core::IoError;
use serde::Deserialize;

use crate::{Error, Result};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    plugins: Vec<PluginEntry>,
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
    rumu_cards::yaml::from_str(&text).map_err(|source| Error::Yaml {
        path,
        source: Box::new(source),
    })
}

impl Config {
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
    }

    #[test]
    fn missing_config_is_the_default_and_unknown_keys_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config = load_config(dir.path()).unwrap();
        assert!(config.external_specs(dir.path()).unwrap().is_empty());

        std::fs::write(dir.path().join("config.yaml"), "bogus: {}\n").unwrap();
        let err = load_config(dir.path()).unwrap_err().to_string();
        assert!(err.contains("unknown field `bogus`"), "{err}");
    }
}
