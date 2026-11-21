//! Repositories of skills installed from git under `RUMU_HOME/skills`. The
//! clone is the record: its `origin` is where it came from.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::Skills;

/// A clone or a pull gives up after this long.
const GIT_TIMEOUT: Duration = Duration::from_secs(300);

impl Skills {
    /// Clone `url` as `name` (by default the URL's `owner-repo`). Returns the
    /// name.
    pub async fn install(&self, url: &str, name: Option<&str>) -> Result<String, String> {
        let url = url.trim();
        if url.is_empty() {
            return Err("no URL".into());
        }
        let name = match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_owned(),
            None => name_of(url).ok_or_else(|| format!("no name in `{url}`; give one"))?,
        };
        if !valid_repo_name(&name) {
            return Err(format!(
                "`{name}` cannot name a repository: 1–64 of [A-Za-z0-9_.-], not starting with `.`"
            ));
        }
        let dir = self.installed.join(&name);
        if dir.exists() {
            return Err(format!("`{name}` is already installed"));
        }
        std::fs::create_dir_all(&self.installed)
            .map_err(|e| format!("creating {}: {e}", self.installed.display()))?;
        let cloned = git(
            &self.installed,
            &["clone", "--quiet", "--depth", "1", "--", url, name.as_str()],
        )
        .await;
        if let Err(e) = cloned {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        Ok(name)
    }

    /// Pull repository `name` (fast-forward only).
    pub async fn update(&self, name: &str) -> Result<(), String> {
        git(&self.repo_dir(name)?, &["pull", "--quiet", "--ff-only"]).await
    }

    /// Delete repository `name`.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let dir = self.repo_dir(name)?;
        std::fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))
    }

    fn repo_dir(&self, name: &str) -> Result<std::path::PathBuf, String> {
        let dir = self.installed.join(name);
        if !valid_repo_name(name) || !dir.is_dir() {
            return Err(format!("no installed repository `{name}`"));
        }
        Ok(dir)
    }
}

fn valid_repo_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A URL's owner and repository, without `.git`: `https://host/o/skills.git`
/// → `o-skills`; the repository alone when there is no valid owner.
fn name_of(url: &str) -> Option<String> {
    let mut segments = url.split(['/', ':']).filter(|s| !s.is_empty()).rev();
    let repo = segments.next()?.trim_end_matches(".git");
    let named = segments
        .next()
        .map(|owner| format!("{owner}-{repo}"))
        .filter(|n| valid_repo_name(n));
    named.or_else(|| valid_repo_name(repo).then(|| repo.to_owned()))
}

/// Run git in `cwd`, never prompting; the error is its stderr's tail.
async fn git(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let run = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(GIT_TIMEOUT, run)
        .await
        .map_err(|_| format!("git {} timed out", args[0]))?
        .map_err(|e| format!("running git: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(5)..].join("\n");
    Err(if tail.is_empty() {
        format!("git {} failed ({})", args[0], out.status)
    } else {
        tail
    })
}

/// The `origin` URL in a clone's `.git/config`.
pub(super) fn origin(dir: &Path) -> Option<String> {
    let config = std::fs::read_to_string(dir.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            in_origin = line == r#"[remote "origin"]"#;
        } else if in_origin
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "url"
        {
            return Some(value.trim().to_owned());
        }
    }
    None
}

#[cfg(test)]
#[test]
fn names_a_url_by_owner_and_repo() {
    for (url, name) in [
        ("https://github.com/o/skills", Some("o-skills")),
        ("https://github.com/o/skills.git/", Some("o-skills")),
        ("git@github.com:o/skills.git", Some("o-skills")),
        ("skills", Some("skills")),
        ("../skills", Some("skills")),
        ("", None),
    ] {
        assert_eq!(name_of(url).as_deref(), name, "{url}");
    }
}
