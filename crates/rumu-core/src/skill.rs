//! What the web sees of the skills on the rumu host (see `docs/skills.md`).

use serde::Serialize;

/// A skill directory found on the rumu host.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SkillInfo {
    /// Its `name` (the directory's name when the frontmatter has none).
    pub name: String,
    pub description: String,
    /// Its directory, `~` for the home directory.
    pub location: String,
    /// The installed repository it is in.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repo: Option<String>,
    /// Why it is not usable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// Another skill of the same name was found first and wins.
    pub shadowed: bool,
}

/// A repository of skills installed from git under `RUMU_HOME/skills`.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SkillRepoInfo {
    pub name: String,
    /// Its `origin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SkillsInfo {
    /// In precedence order: installed repositories, then `~/.agents/skills`,
    /// then `~/.claude/skills`.
    pub skills: Vec<SkillInfo>,
    pub repos: Vec<SkillRepoInfo>,
}
