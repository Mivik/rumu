//! The card content model.
//!
//! A card is a markdown file under `cards/` with optional YAML frontmatter
//! (`title`, `description`; every other key, `scope:` included, is kept
//! verbatim in [`Card::meta`]). Its id is the file path relative to the card root without `.md`. The revision is a
//! deterministic hash of the exact text shown to the model, so metadata-only
//! edits never look like content updates.

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::{CardViewError, yaml};

/// A card as shown to the UI (`card.list`) and used internally.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CardInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Frontmatter `keywords:` — the terms that should bring the card to mind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub body: String,
    pub revision: String,
}

/// One loaded card: parsed frontmatter + body. The raw frontmatter mapping is
/// kept so writes can preserve keys this version does not know about.
#[derive(Debug, Clone)]
pub struct Card {
    pub id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub body: String,
    /// Revision of [`render`] — see [`revision_of`].
    pub revision: String,
    /// Raw frontmatter mapping (unknown keys preserved across rewrites).
    pub meta: Map<String, Value>,
}

impl Card {
    /// The exact text handed to the model. [`revision_of`] hashes exactly
    /// this.
    pub fn render(&self) -> String {
        render(&self.id, &self.title, &self.body)
    }

    /// A one-line summary: the non-empty `description`, else
    /// [`fallback_description`] over the body.
    pub fn summary(&self) -> Option<String> {
        match &self.description {
            Some(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
            _ => fallback_description(&self.body),
        }
    }

    /// Frontmatter `keywords:`: a list of strings, or one comma-separated
    /// string (`a, b, c`, the folded form). In file order; anything else (or
    /// no `keywords:`) is empty.
    pub fn keywords(&self) -> Vec<&str> {
        match self.meta.get("keywords") {
            Some(Value::String(s)) => s
                .split([',', '，'])
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .collect(),
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        }
    }
}

/// The body's first usable line as a short description: skips blank lines,
/// `#` headings and `---` rules, strips a leading `- ` / `* `, and truncates
/// to 100 chars (99 + `…`).
pub fn fallback_description(body: &str) -> Option<String> {
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line == "---" {
            continue;
        }
        let line = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .unwrap_or(line)
            .trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().count() > 100 {
            let mut out: String = line.chars().take(99).collect();
            out.push('…');
            return Some(out);
        }
        return Some(line.to_owned());
    }
    None
}

/// `# {title or id}\n{body}` — the model-facing rendering of a card.
pub fn render(id: &str, title: &Option<String>, body: &str) -> String {
    format!("# {}\n{}", title.as_deref().unwrap_or(id), body)
}

/// Revision = first 4 bytes of SHA-256 of the rendered text, as 8 hex chars.
/// Deterministic across runs and platforms; only covers what the model sees.
pub fn revision_of(rendered: &str) -> String {
    let digest = Sha256::digest(rendered.as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Render a frontmatter mapping as a fenced YAML block (`""` when empty).
fn yaml_block(m: Map<String, Value>) -> String {
    if m.is_empty() {
        return String::new();
    }
    let yaml = serde_saphyr::to_string(&Value::Object(m)).unwrap_or_default();
    format!("---\n{}\n---\n", yaml.trim_end())
}

/// The editable view of a card (what `read` returns and `write`/`edit`
/// accept for `workspace: card`): YAML frontmatter holding `title`,
/// `description`, and every other key, followed by the body. The frontmatter block is omitted when it would be
/// empty.
pub fn view_of(card: &Card) -> String {
    let ystr = |s: &str| Value::String(s.to_owned());
    let mut m = Map::new();
    if let Some(t) = &card.title {
        m.insert("title".into(), ystr(t));
    }
    if let Some(d) = &card.description {
        m.insert("description".into(), ystr(d));
    }
    // Unknown keys ride along in their original order.
    for (k, v) in &card.meta {
        if matches!(k.as_str(), "title" | "description") {
            continue;
        }
        m.insert(k.clone(), v.clone());
    }

    let mut view = yaml_block(m);
    view.push_str(&card.body);
    view
}

/// `view_rev` = the same 8-hex SHA-256 scheme applied to the editable view
/// text (frontmatter included). Two revisions exist per card: `rev` (the
/// rendered text the memory plugin injects) and `view_rev` (what the edit
/// tools read and must have seen before overwriting).
pub fn view_rev_of(view: &str) -> String {
    revision_of(view)
}

/// Parse editable-view text back into a [`Card`], validating as we go (the
/// post-validator of the `write`/`edit` tools: nothing touches disk until it
/// passes). Every frontmatter key is editable. Errors name the offending
/// field.
pub fn card_from_view(id: &str, view: &str) -> Result<Card, CardViewError> {
    let (yaml, body) = split_frontmatter(view);
    let mut meta = Map::new();
    let mut title = None;
    let mut description = None;

    if let Some(yaml) = yaml {
        let parsed: Value = if yaml.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            yaml::from_str(yaml).map_err(CardViewError::InvalidYaml)?
        };
        let Value::Object(map) = parsed else {
            return Err(CardViewError::NotMapping);
        };
        for (key, v) in map {
            match key.as_str() {
                "title" | "description" => match v {
                    Value::Null => {}
                    Value::String(s) => {
                        if key == "title" {
                            title = Some(s);
                        } else {
                            description = Some(s);
                        }
                    }
                    _ => return Err(CardViewError::FieldNotString { field: key }),
                },
                // Unknown keys pass through.
                _ => {
                    meta.insert(key, v);
                }
            }
        }
    }

    // Normalize the trailing newline the way the file will store it.
    let body = body.trim_start_matches('\n');
    let body = if body.is_empty() {
        String::new()
    } else if body.ends_with('\n') {
        body.to_owned()
    } else {
        format!("{body}\n")
    };
    let mut card = Card {
        id: id.to_owned(),
        title,
        description,
        body,
        revision: String::new(),
        meta,
    };
    card.revision = revision_of(&card.render());
    Ok(card)
}

/// Parse a loaded card file into a [`Card`] (computes the revision).
pub fn parse_card(id: &str, text: &str) -> Card {
    let (yaml, body) = split_frontmatter(text);
    let meta_mapping: Map<String, Value> = yaml
        .and_then(|y| yaml::from_str::<Value>(y).ok())
        .map(|v| match v {
            Value::Object(m) => m,
            other => {
                let mut m = Map::new();
                m.insert("<frontmatter>".into(), other);
                m
            }
        })
        .unwrap_or_default();

    let get_str = |key: &str| -> Option<String> {
        meta_mapping
            .get(key)
            .and_then(|v| v.as_str().map(str::to_owned))
    };

    let body = body.trim_start_matches('\n').to_owned();
    let title = get_str("title");
    let revision = revision_of(&render(id, &title, &body));
    Card {
        id: id.to_owned(),
        title,
        description: get_str("description"),
        body,
        revision,
        meta: meta_mapping,
    }
}

/// Serialize a card back to its file content. Known keys are updated in place
/// (preserving their original position); unknown frontmatter keys survive.
pub fn card_content(card: &Card) -> String {
    let mut m = card.meta.clone();
    let ystr = |s: &str| Value::String(s.to_owned());
    let set = |m: &mut Map<String, Value>, key: &str, v: Option<Value>| match v {
        Some(v) => {
            m.insert(key.to_owned(), v);
        }
        None => {
            m.remove(key);
        }
    };
    set(&mut m, "title", card.title.as_deref().map(ystr));
    set(&mut m, "description", card.description.as_deref().map(ystr));

    let mut content = yaml_block(m);
    content.push_str(&card.body);
    if !card.body.ends_with('\n') {
        content.push('\n');
    }
    content
}

/// Split leading `---\n…\n---\n` YAML frontmatter off a card file (or an
/// editable view).
pub fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let rest = match text.strip_prefix("---\n") {
        Some(r) => r,
        None => return (None, text),
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (Some(yaml), body);
        }
        offset += line.len();
    }
    (None, text)
}

/// Card ids are `/`-separated path segments under `cards/`. Each segment is
/// non-empty, must not start with `.` and uses `[A-Za-z0-9._-]` only — so `..`
/// and absolute paths are impossible, and `@` (the key/revision separator)
/// cannot appear. Max 200 chars, 8 levels deep.
pub fn valid_card_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 200 {
        return false;
    }
    let depth = id.split('/').count();
    if depth > 8 {
        return false;
    }
    id.split('/').all(|s| {
        !s.is_empty()
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    })
}

/// Normalize a directory prefix for `list` (workspace `card`) /
/// `search`: strip trailing `/`; the empty string means "everywhere".
pub(crate) fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').to_owned()
}

/// Does `id` live under directory `prefix` (or equal it)?
pub(crate) fn under_prefix(id: &str, prefix: &str) -> bool {
    prefix.is_empty() || id == prefix || id.starts_with(&format!("{prefix}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Card ids follow the id rule; malformed forms are rejected.
    #[test]
    fn ids() {
        for ok in [
            "a",
            "dev",
            "dev/rumu",
            "dev/rumu/kernel",
            "a.b",
            "A-b_9",
            "x.y/z",
        ] {
            assert!(valid_card_id(ok), "should accept `{ok}`");
        }
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "..",
            "../x",
            "a/../b",
            ".h",
            "a/.git/x",
            "a\\b",
            "a@b",
            "a b",
            "a/b@c",
            "@rumu:/card/x",
            "日本語",
        ] {
            assert!(!valid_card_id(bad), "should reject `{bad}`");
        }
        assert!(!valid_card_id(&"a".repeat(201)), "too long");
        assert!(!valid_card_id(&["a"; 9].join("/")), "too deep");
        assert!(valid_card_id(&["a"; 8].join("/")), "depth 8 ok");
    }

    #[test]
    fn frontmatter_roundtrip_and_revision() {
        // The revision is deterministic and only covers the rendered text.
        let a = revision_of("# x\nhello\n");
        assert_eq!(a, revision_of("# x\nhello\n"));
        assert_eq!(a.len(), 8);
        assert_ne!(a, revision_of("# x\nhello v2\n"));
        assert_ne!(a, revision_of("# y\nhello\n"));

        let card = parse_card(
            "m",
            "---\ntitle: Old\ndescription: d\ntags: [a, b]\nmatch:\n  event: prompt\nowner: test\n---\nbody\n",
        );
        assert_eq!(card.title.as_deref(), Some("Old"));
        assert_eq!(card.description.as_deref(), Some("d"));
        // `tags:` is an ordinary unknown key: kept verbatim.
        assert_eq!(card.meta.get("tags"), Some(&serde_json::json!(["a", "b"])));
        // Metadata-only changes do not move the revision.
        assert_eq!(card.render(), "# Old\nbody\n");
        assert_eq!(card.revision, revision_of("# Old\nbody\n"));
        let content = card_content(&card);
        assert!(content.contains("title: Old"), "{content}");
        assert!(content.contains("owner: test"), "{content}");
        assert!(content.contains("tags"), "{content}");
        assert!(content.contains("match:"), "{content}");
        assert!(content.contains("event: prompt"), "{content}");
        assert!(content.ends_with("body\n"));

        // Re-parse: same known fields and revision.
        let again = parse_card("m", &content);
        assert_eq!(again.title, card.title);
        assert_eq!(again.description, card.description);
        assert_eq!(again.body, "body\n");
        assert_eq!(again.revision, card.revision);
    }

    #[test]
    fn plain_and_unterminated_bodies() {
        let card = parse_card("plain", "just body\n");
        assert_eq!(card_content(&card), "just body\n");
        assert!(!card_content(&card).contains("---"));

        // An unterminated frontmatter fence is body text, not metadata.
        let (_, body) = split_frontmatter("---\ntitle: X\n");
        assert!(body.starts_with("---"));
    }

    /// YAML 1.1 boolean words stay strings (strict_booleans), and numeric
    /// values survive a file round-trip.
    #[test]
    fn yaml_booleans_and_numbers_roundtrip() {
        let card = parse_card("b", "---\nowner: y\nflag: on\nnum: 123\n---\nbody\n");
        assert_eq!(card.meta.get("owner"), Some(&Value::String("y".into())));
        assert_eq!(card.meta.get("flag"), Some(&Value::String("on".into())));
        assert_eq!(card.meta.get("num"), Some(&serde_json::json!(123)));
        let again = parse_card("b", &card_content(&card));
        assert_eq!(again.meta, card.meta);
    }

    // ---------- editable view ----------

    #[test]
    fn view_roundtrip_empty_frontmatter_and_rev() {
        let card = parse_card(
            "m",
            "---\ntitle: Old\ndescription: d\ntags: [a, b]\nmatch:\n  event: prompt\nscope: [proj]\nowner: test\n---\nbody\n",
        );
        let view = view_of(&card);
        assert!(
            view.contains("title: Old")
                && view.contains("description: d")
                && view.contains("owner: test"),
            "{view}"
        );
        assert!(view.contains("match:\n  event: prompt"), "{view}");
        assert!(view.contains("scope:"), "{view}");
        assert!(view.contains("tags"), "{view}");
        assert!(view.ends_with("---\nbody\n"), "{view}");

        // Parsing the view back keeps every frontmatter key; the written
        // file carries them unchanged.
        let back = card_from_view("m", &view).unwrap();
        assert_eq!(back.title.as_deref(), Some("Old"));
        assert_eq!(back.body, "body\n");
        for key in ["owner", "match", "scope", "tags"] {
            assert_eq!(back.meta.get(key), card.meta.get(key), "{key}");
        }
        let content = card_content(&back);
        assert!(content.contains("match:"), "{content}");
        assert!(content.contains("event: prompt"), "{content}");
        assert!(content.contains("scope:"), "{content}");
        assert!(content.contains("owner: test"), "{content}");
        assert!(content.contains("tags"), "{content}");
        // And the view of the re-parsed file is byte-identical.
        assert_eq!(view_of(&parse_card("m", &content)), view);

        // No frontmatter → no block.
        assert_eq!(view_of(&parse_card("plain", "just body\n")), "just body\n");

        // `view_rev` hashes the view text.
        let a = view_rev_of("body\n");
        assert_eq!(a.len(), 8);
        assert_eq!(a, view_rev_of("body\n"));
        assert_ne!(a, view_rev_of("---\ntitle: T\n---\nbody\n"));
    }

    #[test]
    fn card_from_view_validates_named_fields() {
        let err = card_from_view("x", "---\ntitle: [a, b]\n---\nbody\n").unwrap_err();
        let err = err.to_string();
        assert!(err.contains("`title`"), "{err}");
        let err = card_from_view("x", "---\ndescription: 3\n---\nbody\n").unwrap_err();
        let err = err.to_string();
        assert!(err.contains("`description`"), "{err}");
        let err = card_from_view("x", "---\n- just\n- a\n- list\n---\nbody\n").unwrap_err();
        assert!(err.to_string().contains("mapping"), "{err}");
        // Null title clears it; unknown keys (tags included) pass through.
        let card = card_from_view("x", "---\ntitle: null\ntags: not-a-list\n---\nbody\n").unwrap();
        assert_eq!(card.title, None);
        assert_eq!(
            card.meta.get("tags"),
            Some(&Value::String("not-a-list".into()))
        );
        assert!(card_content(&card).contains("tags"));
        // No frontmatter at all: metadata-less card.
        let card = card_from_view("x", "body only\n").unwrap();
        assert_eq!((card.title, card.description), (None, None));
        assert_eq!(card.body, "body only\n");
    }

    // ---------- summary ----------

    #[test]
    fn fallback_description_skips_noise_and_truncates() {
        assert_eq!(fallback_description(""), None);
        assert_eq!(fallback_description("\n\n# heading\n---\n"), None);
        assert_eq!(
            fallback_description("# heading\n\n- the first line\nsecond\n"),
            Some("the first line".into())
        );
        assert_eq!(fallback_description("\n* bullet\n"), Some("bullet".into()));
        // Over 100 chars: 99 chars + ellipsis (char-counted).
        let long = "日".repeat(120);
        let out = fallback_description(&format!("{long}\n")).unwrap();
        assert_eq!(out.chars().count(), 100);
        assert!(out.ends_with('…'));
        assert!(out.starts_with('日'));
    }

    #[test]
    fn summary_prefers_description_then_falls_back() {
        let mut c = parse_card("x", "---\ndescription:  the gist \n---\nbody line\n");
        assert_eq!(c.summary().as_deref(), Some("the gist"));
        c.description = Some("   ".into());
        assert_eq!(c.summary().as_deref(), Some("body line"));
        c.body = "\n# only a heading\n".into();
        assert_eq!(c.summary(), None);
    }

    // ---------- keywords ----------

    #[test]
    fn keywords_are_a_string_or_a_string_list() {
        let one = parse_card("k", "---\nkeywords: deploy\n---\nbody\n");
        assert_eq!(one.keywords(), ["deploy"]);
        // A lone string holds the whole list, comma-separated (the folded
        // form the memory prompt's list becomes in the file).
        let folded = parse_card("k", "---\nkeywords: deploy, k8s , rollout\n---\nbody\n");
        assert_eq!(folded.keywords(), ["deploy", "k8s", "rollout"]);
        let fullwidth = parse_card("k", "---\nkeywords: 部署，发布\n---\nbody\n");
        assert_eq!(fullwidth.keywords(), ["部署", "发布"]);
        // A folded block scalar joins its lines with spaces, so a phrase
        // stays one keyword.
        let long = parse_card(
            "k",
            "---\nkeywords: >-\n  deploy, Arch\n  Linux\n---\nbody\n",
        );
        assert_eq!(long.keywords(), ["deploy", "Arch Linux"]);
        // Non-string entries are skipped; order is kept.
        let many = parse_card("k", "---\nkeywords: [deploy, 2, rollout]\n---\nbody\n");
        assert_eq!(many.keywords(), ["deploy", "rollout"]);
        // Anything else is no keywords at all.
        let none = parse_card("k", "---\nkeywords: 2\n---\nbody\n");
        assert!(none.keywords().is_empty());
        assert!(
            parse_card("k", "---\nkeywords: \" , \"\n---\nbody\n")
                .keywords()
                .is_empty()
        );
        assert!(parse_card("k", "body\n").keywords().is_empty());
    }
}
