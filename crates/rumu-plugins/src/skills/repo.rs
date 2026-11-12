//! Repositories of skills under `RUMU_HOME/skills`: each a git clone,
//! whose `origin` is where it came from.

use std::path::Path;

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
