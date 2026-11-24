//! `RUMU_HOME/workspaces.yaml`: the workspace registry, and
//! `RUMU_HOME/projects.yaml`: the workspaces each project mounts. rumu is
//! the only writer of both (the web UI and the model's `@rumu:/workspaces`
//! writes). A registry entry is the string shorthand (`proj: ~/code/proj`) or
//! `{root, ssh?, read_only?}`; a project entry is `<deck id>: [<name>, …]`.

use std::collections::BTreeMap;
use std::path::Path;

use rumu_core::{
    IoError, ProjectWorkspaces, RUMU_WORKSPACE, Workspace, Workspaces, valid_workspace_name,
};
use serde::{Deserialize, Deserializer, Serialize, de};

use rumu_cards::{Deck, DeckIcon, DeckMeta, Decks};

use crate::{Error, Result};

pub const WORKSPACES_FILE: &str = "workspaces.yaml";
pub const PROJECTS_FILE: &str = "projects.yaml";
/// Sessions' scratch directories, one per session id.
pub const SCRATCH_DIR: &str = "scratch";

/// Load the registry. With no file at all, the current directory becomes
/// the one workspace and the file is written (zero-config first start).
/// Invalid names, bad ssh targets, non-absolute ssh roots and unknown keys
/// are errors: a workspace that silently vanishes is worse than a startup
/// failure.
pub fn load_workspaces(home: &Path) -> Result<Workspaces> {
    let list = load_list(home)?;
    let projects = load_projects(home)?;
    Ok(Workspaces::with_projects(list, projects).with_scratch(home.join(SCRATCH_DIR)))
}

fn load_list(home: &Path) -> Result<Vec<Workspace>> {
    let path = home.join(WORKSPACES_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let cwd = std::env::current_dir().map_err(|source| {
                Error::Io(IoError::op("resolving the current directory", source))
            })?;
            let root = cwd.to_string_lossy().into_owned();
            let list = vec![Workspace::new(cwd_workspace_name(&root), None, root, false)];
            save(home, &list)?;
            return Ok(list);
        }
        Err(source) => return Err(IoError::at("reading", &path)(source).into()),
    };
    let entries: Option<BTreeMap<String, Entry>> =
        rumu_cards::yaml::from_str(&text).map_err(|source| Error::Yaml {
            path,
            source: Box::new(source),
        })?;
    entries
        .unwrap_or_default()
        .into_iter()
        .map(|(name, e)| check(name, e))
        .collect()
}

/// The project lists; a missing file is none. Names are kept as written: one
/// that leaves the registry drops out when read (see [`Workspaces::project`]).
fn load_projects(home: &Path) -> Result<ProjectWorkspaces> {
    let path = home.join(PROJECTS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(source) => return Err(IoError::at("reading", &path)(source).into()),
    };
    let projects: Option<ProjectWorkspaces> =
        rumu_cards::yaml::from_str(&text).map_err(|source| Error::Yaml {
            path,
            source: Box::new(source),
        })?;
    Ok(projects.unwrap_or_default())
}

/// Make `names` (registered, each once) the workspaces `project` mounts, in
/// order, and save. An empty list drops the project's entry.
pub fn set_project_workspaces(
    home: &Path,
    registry: &Workspaces,
    project: &str,
    names: Vec<String>,
) -> Result<()> {
    registry.update_both(|list, projects| {
        for (i, name) in names.iter().enumerate() {
            let fail = |message: &str| Error::WorkspaceConfig {
                name: name.clone(),
                message: message.into(),
            };
            if !list.iter().any(|w| &w.name == name) {
                return Err(fail("no such workspace"));
            }
            if names[..i].contains(name) {
                return Err(fail("listed twice"));
            }
        }
        if names.is_empty() {
            projects.remove(project);
        } else {
            projects.insert(project.to_owned(), names);
        }
        save_projects(home, projects)
    })
}

/// Forget the workspaces of `project` (its deck was deleted), and save.
pub fn drop_project(home: &Path, registry: &Workspaces, project: &str) -> Result<()> {
    if !registry.projects().contains_key(project) {
        return Ok(());
    }
    registry.update_both(|_, projects| {
        projects.remove(project);
        save_projects(home, projects)
    })
}

/// Add `ws`, or replace the workspace of the same name, and save.
pub fn upsert_workspace(home: &Path, registry: &Workspaces, ws: Workspace) -> Result<Workspace> {
    let entry = Entry {
        root: ws.root.as_str().to_owned(),
        ssh: ws.ssh,
        read_only: ws.read_only,
    };
    let ws = check(ws.name, entry)?;
    registry.update(|list| {
        list.retain(|w| w.name != ws.name);
        list.push(ws.clone());
        save(home, list)
    })?;
    Ok(ws)
}

/// Remove the workspace `name`, and from every project that mounts it, and
/// save.
pub fn remove_workspace(home: &Path, registry: &Workspaces, name: &str) -> Result<()> {
    registry.update_both(|list, projects| {
        let before = list.len();
        list.retain(|w| w.name != name);
        if list.len() == before {
            return Err(Error::WorkspaceConfig {
                name: name.to_owned(),
                message: "no such workspace".into(),
            });
        }
        let mounted = projects
            .values()
            .any(|names| names.iter().any(|n| n == name));
        if mounted {
            for names in projects.values_mut() {
                names.retain(|n| n != name);
            }
            projects.retain(|_, names| !names.is_empty());
            save_projects(home, projects)?;
        }
        save(home, list)
    })
}

/// Write `projects.yaml` whole, as [`save`] does.
fn save_projects(home: &Path, projects: &ProjectWorkspaces) -> Result<()> {
    let path = home.join(PROJECTS_FILE);
    let text = serde_saphyr::to_string(projects).map_err(|e| Error::Config {
        path: path.clone(),
        message: format!("serializing: {e}"),
    })?;
    write_whole(home, PROJECTS_FILE, &text)
}

/// Write the file whole (through a temp file, so a crash never leaves half
/// of it).
fn save(home: &Path, list: &[Workspace]) -> Result<()> {
    let entries: BTreeMap<&str, Entry> = list
        .iter()
        .map(|w| {
            let entry = Entry {
                root: w.root.as_str().to_owned(),
                ssh: w.ssh.clone(),
                read_only: w.read_only,
            };
            (w.name.as_str(), entry)
        })
        .collect();
    let path = home.join(WORKSPACES_FILE);
    let text = serde_saphyr::to_string(&entries).map_err(|e| Error::Config {
        path: path.clone(),
        message: format!("serializing: {e}"),
    })?;
    write_whole(home, WORKSPACES_FILE, &text)
}

fn write_whole(home: &Path, file: &str, text: &str) -> Result<()> {
    let path = home.join(file);
    let tmp = home.join(format!("{file}.tmp"));
    std::fs::write(&tmp, text).map_err(IoError::at("writing", &tmp))?;
    std::fs::rename(&tmp, &path).map_err(IoError::at("renaming", &tmp))?;
    Ok(())
}

/// One project deck as the web sidebar sees it.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// The one-line summary from its charter (`about.md`).
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The workspaces it mounts, in order (the first is its default).
    pub workspaces: Vec<String>,
    /// Its seal, when set.
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<DeckIcon>,
    /// Hidden from agents unless this session has it mounted.
    pub private: bool,
}

/// Every project deck, by name.
pub fn projects(decks: &Decks, registry: &Workspaces) -> Vec<Project> {
    let mut list: Vec<Project> = decks
        .projects()
        .iter()
        .map(|d| project_of(d, registry))
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

/// The wire form of one project deck.
pub fn project_of(deck: &Deck, registry: &Workspaces) -> Project {
    Project {
        id: deck.id().to_owned(),
        name: deck.meta.name.clone(),
        description: deck.summary(),
        workspaces: registry.project(deck.id()),
        icon: deck.meta.icon.clone(),
        private: deck.private(),
    }
}

/// Create a project deck named `name` (blank charter) mounting `workspaces`.
pub fn create_project(
    home: &Path,
    decks: &Decks,
    registry: &Workspaces,
    name: &str,
    workspaces: Vec<String>,
    icon: Option<DeckIcon>,
    private: bool,
) -> Result<Deck> {
    let deck = decks.upsert(DeckMeta {
        id: String::new(),
        name: name.to_owned(),
        assistant: false,
        project: true,
        private,
        mounts: Vec::new(),
        allow_ask: Vec::new(),
        model: None,
        mcp: Default::default(),
        skills: Vec::new(),
        icon,
        cat: None,
    })?;
    set_project_workspaces(home, registry, deck.id(), workspaces)?;
    Ok(deck)
}

/// A fresh home's first project: the current directory's workspace (the only
/// one registered), in a project deck of the same name. Skipped when a deck
/// of that name already exists, or when the directory is the user's home:
/// that is where rumu starts by default, not a project.
pub fn seed_project(home: &Path, decks: &Decks, registry: &Workspaces) -> Result<()> {
    let Ok([ws]) = <[Workspace; 1]>::try_from(registry.list()) else {
        return Ok(());
    };
    let user_home = std::env::home_dir();
    if ws.ssh.is_none() && user_home.is_some_and(|h| h == Path::new(ws.root.as_str())) {
        return Ok(());
    }
    let name = ws.name.clone();
    if !rumu_cards::deck::valid_deck_name(&name) || decks.name_taken(&name, None) {
        return Ok(());
    }
    create_project(home, decks, registry, &name, vec![ws.name], None, false)?;
    Ok(())
}

/// One entry: the string shorthand for a local read-write root, or the map
/// form. Always written as the map form.
#[derive(Debug, Serialize)]
struct Entry {
    root: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ssh: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    read_only: bool,
}

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Map {
            root: String,
            // `ssh: null` (YAML) is the same as omitting it.
            #[serde(default)]
            ssh: Option<String>,
            #[serde(default)]
            read_only: bool,
        }
        let v = serde_json::Value::deserialize(d)?;
        match v {
            serde_json::Value::String(root) => Ok(Self {
                root,
                ssh: None,
                read_only: false,
            }),
            serde_json::Value::Object(_) => {
                let m = Map::deserialize(v).map_err(de::Error::custom)?;
                Ok(Self {
                    root: m.root,
                    ssh: m.ssh,
                    read_only: m.read_only,
                })
            }
            _ => Err(de::Error::custom(
                "a workspace is a root path string or a `{root, ssh?, read_only?}` map",
            )),
        }
    }
}

/// Parse and validate one registry entry (`@rumu:/workspaces/<name>`): the
/// string shorthand or the `{root, ssh?, read_only?}` map, with a local
/// `root` required to be an existing directory (after `~` expansion).
pub(crate) fn parse_entry(name: &str, text: &str) -> Result<Workspace> {
    let entry: Entry =
        rumu_cards::yaml::from_str(text).map_err(|source| Error::WorkspaceConfig {
            name: name.to_owned(),
            message: format!("does not parse: {source}"),
        })?;
    let ws = check(name.to_owned(), entry)?;
    if ws.is_local() && !Path::new(ws.root.as_str()).is_dir() {
        return Err(Error::WorkspaceConfig {
            name: name.to_owned(),
            message: format!("root `{}` is not an existing directory", ws.root),
        });
    }
    Ok(ws)
}

/// One registry entry's canonical YAML text, as `@rumu:/workspaces/<name>`
/// reads it.
pub(crate) fn entry_yaml(ws: &Workspace) -> Result<String> {
    let entry = Entry {
        root: ws.root.as_str().to_owned(),
        ssh: ws.ssh.clone(),
        read_only: ws.read_only,
    };
    serde_saphyr::to_string(&entry).map_err(|e| Error::WorkspaceConfig {
        name: ws.name.clone(),
        message: format!("serializing: {e}"),
    })
}

/// Validate one entry into a workspace. `~` in local roots expands to
/// `$HOME`; an empty `ssh` is local.
fn check(name: String, entry: Entry) -> Result<Workspace> {
    let fail = |message: String| Error::WorkspaceConfig {
        name: name.clone(),
        message,
    };
    if !valid_workspace_name(&name) {
        return Err(fail(format!(
            "invalid workspace name (rule: `[a-z][a-z0-9.-]*`, length >= 2; the common URI \
             schemes are reserved; `{RUMU_WORKSPACE}` is reserved for rumu's own data)"
        )));
    }
    let root = entry.root.trim();
    if root.is_empty() {
        return Err(fail("workspace root must be non-empty".into()));
    }
    let Some(ssh) = entry.ssh.filter(|s| !s.is_empty()) else {
        // `~` means $HOME — only on the machine rumu runs on.
        return Ok(Workspace::new(
            name.clone(),
            None,
            expand_tilde(root),
            entry.read_only,
        ));
    };
    if ssh.chars().any(char::is_whitespace) {
        return Err(fail(format!(
            "ssh host must contain no whitespace (got `{ssh}`)"
        )));
    }
    // An ssh root must be absolute: there is no local cwd to resolve
    // against, and a silent miss is worse than a load error.
    if !root.starts_with('/') {
        return Err(fail(format!(
            "ssh `root` must be an absolute path (got `{root}`)"
        )));
    }
    Ok(Workspace::new(
        name.clone(),
        Some(ssh),
        root.to_owned(),
        entry.read_only,
    ))
}

/// A workspace name for the directory `root`: its basename, lowercased with
/// other characters folded to `-`; an unusable basename becomes `ws`.
pub fn cwd_workspace_name(root: &str) -> String {
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

fn expand_tilde(s: &str) -> String {
    let home = std::env::home_dir().unwrap_or_default();
    match s.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
            format!("{}{rest}", home.display())
        }
        _ => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_text(text: &str) -> Result<Vec<Workspace>> {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(WORKSPACES_FILE), text).unwrap();
        load_workspaces(dir.path()).map(|r| r.list())
    }

    #[test]
    fn entries_parse_in_both_forms_and_are_validated() {
        let list = load_text(
            "proj: ~/code/proj\napp:\n  ssh: deploy@prod\n  root: /srv/app\n  read_only: true\nrumu: /x\n",
        )
        .unwrap();
        let names: Vec<&str> = list.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(
            names,
            ["app", "proj", "rumu"],
            "name order; `rumu` is ordinary"
        );
        let proj = &list[1];
        assert!(proj.is_local() && !proj.read_only);
        assert!(
            proj.root.as_str().ends_with("/code/proj"),
            "tilde expanded: {}",
            proj.root
        );
        let app = &list[0];
        assert_eq!(app.label(), "ssh:deploy@prod");
        assert_eq!(app.root, "/srv/app");
        assert!(app.read_only);

        assert!(
            load_text("").unwrap().is_empty(),
            "an empty file is no workspaces"
        );
        assert!(
            load_text("- /srv/a\n").is_err(),
            "a list is not a workspace map"
        );
        for bad in ["bad/name", "UPPER", "x", "@rumu", "http"] {
            let err = load_text(&format!("\"{bad}\": /x\n"))
                .unwrap_err()
                .to_string();
            assert!(err.contains("invalid workspace name"), "{bad}: {err}");
        }
        for (yaml, needle) in [
            (
                "ab:\n  root: /r\n  machine: ghost\n",
                "unknown field `machine`",
            ),
            ("ab:\n  ssh: 'deploy @prod'\n  root: /r\n", "no whitespace"),
            (
                "ab:\n  ssh: deploy@prod\n  root: srv/app\n",
                "must be an absolute path",
            ),
            ("ab: ''\n", "must be non-empty"),
        ] {
            let err = load_text(yaml).unwrap_err().to_string();
            assert!(err.contains(needle), "{yaml}: {err}");
        }
    }

    #[test]
    fn a_missing_file_registers_the_current_directory() {
        let dir = tempfile::tempdir().unwrap();
        let list = load_workspaces(dir.path()).unwrap().list();
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].root.as_str(), cwd.to_string_lossy());
        assert!(valid_workspace_name(&list[0].name), "{}", list[0].name);
        assert!(dir.path().join(WORKSPACES_FILE).is_file(), "and saved");
    }

    #[test]
    fn upsert_and_remove_save_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(WORKSPACES_FILE), "{}\n").unwrap();
        let registry = load_workspaces(dir.path()).unwrap();
        let app = Workspace::new(
            "app".into(),
            Some("deploy@prod".into()),
            "/srv/app".into(),
            true,
        );
        upsert_workspace(dir.path(), &registry, app.clone()).unwrap();
        upsert_workspace(
            dir.path(),
            &registry,
            Workspace::new("proj".into(), Some(String::new()), "/p".into(), false),
        )
        .unwrap();
        // Replacing keeps one entry per name.
        let proj = Workspace::new("proj".into(), None, "/q".into(), false);
        upsert_workspace(dir.path(), &registry, proj.clone()).unwrap();
        assert_eq!(
            load_workspaces(dir.path()).unwrap().list(),
            [app.clone(), proj]
        );

        let bad = Workspace::new("ab".into(), Some("h".into()), "rel".into(), false);
        assert!(upsert_workspace(dir.path(), &registry, bad).is_err());

        remove_workspace(dir.path(), &registry, "proj").unwrap();
        assert!(remove_workspace(dir.path(), &registry, "proj").is_err());
        assert_eq!(registry.list(), std::slice::from_ref(&app));
        assert_eq!(load_workspaces(dir.path()).unwrap().list(), [app]);
    }
}
