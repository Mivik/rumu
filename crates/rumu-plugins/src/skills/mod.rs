//! Skills: directories with a `SKILL.md` (the agentskills.io format) on the
//! rumu host — in repositories installed from git under `RUMU_HOME/skills`,
//! then in `~/.agents/skills` and `~/.claude/skills`. An assistant and a
//! project enable them by rules; the model sees each enabled one's name and
//! description in a first-call note, and reads it under `@rumu:/skills`
//! (served by the workspace plugin through the functions here). See
//! `docs/skills.md`.

mod repo;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use rumu_cards::Decks;
use rumu_core::{
    Content, Cx, Decision, Event, HookOutput, HookSpec, Manifest, PathState, Plugin,
    RUMU_WORKSPACE, SkillInfo, SkillRepoInfo, SkillsInfo, Structured, ToolError, ToolOutput,
    ToolResult, Utf8UnixPath, any_rules_enable,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Installed repositories, under `RUMU_HOME`.
pub const SKILLS_DIR: &str = "skills";
const SKILL_FILE: &str = "SKILL.md";
const INTRO_HOOK: &str = "skills-intro";
/// Files listed under a skill at most.
const FILES_LIMIT: usize = 200;

/// Payload of the `skills.intro` note (v1): the skill list, again whenever it
/// changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SkillsIntro {
    pub skills: Vec<String>,
    /// Each skill's one-line description, by name, for the UI; absent in
    /// notes written before it existed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub about: BTreeMap<String, String>,
}

impl Structured for SkillsIntro {
    const TAG: &'static str = "skills.intro";
    const V: u32 = 1;
}

/// Where skills are found. Each lookup scans the directories again: they are
/// small, and other tools install into them.
pub struct Skills {
    /// `RUMU_HOME/skills`: one installed repository per directory.
    installed: PathBuf,
    /// Directories of skills, scanned after the repositories.
    dirs: Vec<PathBuf>,
    /// Shown as `~` in locations.
    home: Option<PathBuf>,
}

/// One skill directory.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    /// The installed repository it is in.
    pub repo: Option<String>,
    /// Why it is not usable.
    pub error: Option<String>,
    /// A skill of the same name was found first.
    pub shadowed: bool,
}

impl Skill {
    fn usable(&self) -> bool {
        self.error.is_none() && !self.shadowed
    }
}

impl Skills {
    /// The installed repositories of `rumu_home`, then `~/.agents/skills` and
    /// `~/.claude/skills`.
    pub fn new(rumu_home: &Path) -> Self {
        let home = std::env::home_dir();
        let dirs = home
            .iter()
            .flat_map(|h| [h.join(".agents/skills"), h.join(".claude/skills")])
            .collect();
        Self::with_dirs(rumu_home, dirs, home)
    }

    pub fn with_dirs(rumu_home: &Path, dirs: Vec<PathBuf>, home: Option<PathBuf>) -> Self {
        Self {
            installed: rumu_home.join(SKILLS_DIR),
            dirs,
            home,
        }
    }

    /// Every skill directory in precedence order, the unusable and the
    /// shadowed ones included.
    pub fn scan(&self) -> Vec<Skill> {
        let mut found = Vec::new();
        for (repo, dir) in self.repos() {
            let mut dirs = Vec::new();
            find(&dir, 3, &mut dirs);
            found.extend(dirs.iter().map(|d| load(d, Some(repo.clone()))));
        }
        for dir in &self.dirs {
            let mut dirs = Vec::new();
            find(dir, 2, &mut dirs);
            found.extend(dirs.iter().map(|d| load(d, None)));
        }
        let mut seen = HashSet::new();
        for skill in &mut found {
            if skill.error.is_none() && !seen.insert(skill.name.clone()) {
                skill.shadowed = true;
            }
        }
        found
    }

    /// The usable skills any of the rule lists `rules` enables.
    pub fn enabled(&self, rules: &[Vec<String>]) -> Vec<Skill> {
        if rules.iter().all(Vec::is_empty) {
            return Vec::new();
        }
        self.scan()
            .into_iter()
            .filter(|s| s.usable() && any_rules_enable(rules, &s.name))
            .collect()
    }

    /// The installed repositories, by name.
    fn repos(&self) -> Vec<(String, PathBuf)> {
        let Ok(entries) = std::fs::read_dir(&self.installed) else {
            return Vec::new();
        };
        let mut repos: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
            .filter(|(name, _)| !name.starts_with('.'))
            .collect();
        repos.sort();
        repos
    }

    /// What Settings shows.
    pub fn info(&self) -> SkillsInfo {
        SkillsInfo {
            skills: self
                .scan()
                .into_iter()
                .map(|s| SkillInfo {
                    location: self.display(&s.dir),
                    name: s.name,
                    description: s.description,
                    repo: s.repo,
                    error: s.error,
                    shadowed: s.shadowed,
                })
                .collect(),
            repos: self
                .repos()
                .into_iter()
                .map(|(name, dir)| SkillRepoInfo {
                    url: repo::origin(&dir),
                    name,
                })
                .collect(),
        }
    }

    fn display(&self, dir: &Path) -> String {
        match self.home.as_deref().and_then(|h| dir.strip_prefix(h).ok()) {
            Some(rest) => format!("~/{}", rest.display()),
            None => dir.display().to_string(),
        }
    }
}

/// The directories under `dir` (itself included) with a `SKILL.md`, at most
/// `depth` levels down; a skill's own subdirectories are not searched.
fn find(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(SKILL_FILE).is_file() {
        out.push(dir.to_owned());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            !name.starts_with('.') && name != "node_modules"
        })
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        find(&child, depth - 1, out);
    }
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

/// Read one skill directory. Lenient, as skills are written for other
/// harnesses: a missing `name` is the directory's, and a frontmatter that is
/// not YAML is read line by line; only a missing description makes it
/// unusable.
fn load(dir: &Path, repo: Option<String>) -> Skill {
    let mut skill = Skill {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        description: String::new(),
        dir: dir.to_owned(),
        repo,
        error: None,
        shadowed: false,
    };
    let front = std::fs::read_to_string(dir.join(SKILL_FILE))
        .map_err(|e| format!("reading {SKILL_FILE}: {e}"))
        .and_then(|text| frontmatter(&text));
    match front {
        Err(e) => skill.error = Some(e),
        Ok(front) => {
            if let Some(name) = front
                .name
                .as_deref()
                .map(str::trim)
                .filter(|n| !n.is_empty())
            {
                skill.name = name.to_owned();
            }
            skill.description = front.description.unwrap_or_default().trim().to_owned();
            if skill.description.is_empty() {
                skill.error = Some(format!("{SKILL_FILE} has no description"));
            } else if !valid_skill_name(&skill.name) {
                skill.error = Some(format!("`{}` cannot name a skill", skill.name));
            }
        }
    }
    skill
}

fn frontmatter(text: &str) -> Result<Frontmatter, String> {
    let (Some(yaml), _) = rumu_cards::card::split_frontmatter(text) else {
        return Err(format!("{SKILL_FILE} has no frontmatter"));
    };
    match rumu_cards::yaml::from_str::<Option<Frontmatter>>(yaml) {
        Ok(front) => Ok(front.unwrap_or_default()),
        Err(e) => {
            // Often an unquoted value with `: ` in it, which laxer parsers take.
            let mut front = Frontmatter::default();
            for line in yaml.lines() {
                if let Some(v) = line.strip_prefix("name:") {
                    front.name = Some(v.trim().to_owned());
                } else if let Some(v) = line.strip_prefix("description:") {
                    front.description = Some(v.trim().to_owned());
                }
            }
            match front.description {
                Some(_) => Ok(front),
                None => Err(format!("{SKILL_FILE} frontmatter: {e}")),
            }
        }
    }
}

/// Whether `name` can name a skill in paths and rules: 1–64 letters, digits
/// and `-_.`, not starting with `.`. Laxer than the format's lowercase and
/// hyphens, which not every skill keeps to.
fn valid_skill_name(name: &str) -> bool {
    (1..=64).contains(&name.chars().count())
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A description on one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The skill-list note.
pub struct SkillsPlugin {
    skills: Arc<Skills>,
    decks: Decks,
}

impl SkillsPlugin {
    pub fn new(skills: Arc<Skills>, decks: Decks) -> Self {
        Self { skills, decks }
    }
}

#[async_trait]
impl Plugin for SkillsPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            name: "skills".into(),
            instructions: None,
            tools: Vec::new(),
            hooks: vec![HookSpec {
                id: INTRO_HOOK.into(),
                matcher: json!({ "event": "llm.pre" }),
                priority: i32::MAX - 4,
            }],
        }
    }

    async fn on_hook(
        &self,
        hook_id: &str,
        _event: &Event,
        cx: &Cx,
    ) -> rumu_core::Result<HookOutput> {
        if hook_id != INTRO_HOOK {
            return Ok(HookOutput::default());
        }
        let state = cx.state()?;
        let skills = self.skills.enabled(&rules(&self.decks, &state));
        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        let last = cx
            .index()?
            .all::<SkillsIntro>()
            .pop()
            .map(|e| e.value.skills);
        if last.as_ref().unwrap_or(&Vec::new()) == &names {
            return Ok(HookOutput::default());
        }
        let mut text = if skills.is_empty() {
            "This session has no skills any more.".to_owned()
        } else {
            format!(
                "{}Skills: instructions for specific tasks, read on demand. When a task matches a \
skill's description, `read {RUMU_WORKSPACE}:/skills/<name>` (its {SKILL_FILE}, then a list of its \
files) before going on. Its relative paths are under `{RUMU_WORKSPACE}:/skills/<name>/`; to run \
one of its scripts, write the script into your workspace first.",
                if last.is_some() {
                    "The skills changed; this replaces the earlier list.\n"
                } else {
                    ""
                }
            )
        };
        let mut about = BTreeMap::new();
        for s in &skills {
            let line = one_line(&s.description);
            let _ = write!(text, "\n- `{}`: {line}", s.name);
            about.insert(s.name.clone(), line);
        }
        Ok(HookOutput {
            effects: vec![Content::note_with(
                text,
                &SkillsIntro {
                    skills: names,
                    about,
                },
            )],
            decision: Decision::Pass,
        })
    }
}

/// The skill rules of the session's assistant and of its project, from
/// their decks as they are now.
pub fn rules(decks: &Decks, state: &PathState) -> Vec<Vec<String>> {
    crate::decks::binding(decks, state)
        .into_iter()
        .map(|meta| meta.skills)
        .collect()
}

/// The enabled skill `name` and the path `sub` under it (`abs` names them in
/// errors).
fn locate(
    skills: &Skills,
    rules: &[Vec<String>],
    abs: &Utf8UnixPath,
    name: &str,
    sub: &str,
) -> ToolResult<(Skill, PathBuf)> {
    if sub.split('/').any(|c| c == "..") {
        return Err(ToolError::model(format!(
            "`{RUMU_WORKSPACE}:{abs}` leaves its skill"
        )));
    }
    let skill = skills
        .enabled(rules)
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| {
            ToolError::model(format!(
                "no skill `{name}` in this session; see `list {RUMU_WORKSPACE}:/skills`"
            ))
        })?;
    let path = skill.dir.join(sub);
    Ok((skill, path))
}

/// `list @rumu:/skills`: the session's skills.
pub fn list_skills(skills: &Skills, rules: &[Vec<String>]) -> ToolOutput {
    let enabled = skills.enabled(rules);
    if enabled.is_empty() {
        return "This session has no skills.".into();
    }
    let mut out = String::new();
    for s in enabled {
        let _ = writeln!(out, "{}/ — {}", s.name, one_line(&s.description));
    }
    out.into()
}

/// `list @rumu:/skills/<name>[/<sub>]`: a skill's files.
pub fn list(
    skills: &Skills,
    rules: &[Vec<String>],
    abs: &Utf8UnixPath,
    name: &str,
    sub: &str,
) -> ToolResult<ToolOutput> {
    let (skill, path) = locate(skills, rules, abs, name, sub)?;
    if path.is_file() {
        let rel = path.strip_prefix(&skill.dir).unwrap_or(&path);
        return Ok(format!("{}\n", rel.display()).into());
    }
    if !path.is_dir() {
        return Err(not_found(abs));
    }
    Ok(files(&path).into())
}

/// `read @rumu:/skills/<name>[/<sub>]`: a file; a skill is its `SKILL.md`
/// and a list of its other files; a directory lists.
pub fn read(
    skills: &Skills,
    rules: &[Vec<String>],
    abs: &Utf8UnixPath,
    name: &str,
    sub: &str,
) -> ToolResult<ToolOutput> {
    let (skill, path) = locate(skills, rules, abs, name, sub)?;
    if path == skill.dir {
        let text = read_text(&path.join(SKILL_FILE))?;
        let others: Vec<String> = files_of(&path)
            .0
            .into_iter()
            .filter(|f| f != SKILL_FILE)
            .collect();
        if others.is_empty() {
            return Ok(text.into());
        }
        let listing = files(&path);
        return Ok(format!(
            "{}\n\n---\nIts files, under `{RUMU_WORKSPACE}:/skills/{}/`:\n{}",
            text.trim_end(),
            skill.name,
            listing.trim_end()
        )
        .into());
    }
    if path.is_dir() {
        return Ok(files(&path).into());
    }
    if !path.is_file() {
        return Err(not_found(abs));
    }
    Ok(read_text(&path)?.into())
}

fn not_found(abs: &Utf8UnixPath) -> ToolError {
    ToolError::model(format!("`{RUMU_WORKSPACE}:{abs}` does not exist"))
}

fn read_text(path: &Path) -> ToolResult<String> {
    let bytes = std::fs::read(path)
        .map_err(|e| ToolError::model(format!("reading {}: {e}", path.display())))?;
    String::from_utf8(bytes).map_err(|e| {
        ToolError::model(format!(
            "a binary file ({} bytes), at {} on the rumu host",
            e.as_bytes().len(),
            path.display()
        ))
    })
}

/// The files under `dir`, relative to it, sorted; and how many more there
/// are beyond [`FILES_LIMIT`].
fn files_of(dir: &Path) -> (Vec<String>, usize) {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>, more: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name() != ".git" && e.file_name() != "node_modules")
            .map(|e| e.path())
            .collect();
        paths.sort();
        for p in paths {
            if p.is_dir() {
                walk(root, &p, out, more);
            } else if out.len() < FILES_LIMIT {
                let rel = p.strip_prefix(root).unwrap_or(&p);
                out.push(rel.to_string_lossy().replace('\\', "/"));
            } else {
                *more += 1;
            }
        }
    }
    let (mut out, mut more) = (Vec::new(), 0);
    walk(dir, dir, &mut out, &mut more);
    (out, more)
}

fn files(dir: &Path) -> String {
    let (files, more) = files_of(dir);
    let mut out = files.join("\n");
    if more > 0 {
        let _ = write!(out, "\n… {more} more");
    }
    out.push('\n');
    out
}
