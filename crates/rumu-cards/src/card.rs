//! The card content model.
//!
//! A card is a markdown file in a deck. Its frontmatter `id:` is its identity
//! across moves; its path is where it currently lives. The model never sees
//! the id: [`view_of`] omits it. The revision is a deterministic hash of the
//! exact text shown to the model, so metadata-only edits never look like
//! content updates.

use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use rumu_core::Structured;

use crate::{CardViewError, yaml};

/// A card as shown to the UI (`card.list`) and used internally.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CardInfo {
    /// The card's location: the path within its mounted deck.
    pub id: String,
    /// The id of the deck holding the card file itself (for a link card,
    /// the link file's deck, not its target's).
    pub deck: String,
    /// The deck-local path of the card file itself (no `.md`).
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Frontmatter `keywords:` — the terms that should bring the card to mind.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    pub body: String,
    pub revision: String,
    /// A pinned card is loaded into context when its deck is mounted.
    #[serde(default)]
    pub pinned: bool,
    /// A link card's target, stored as `<deck-id>:<path>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Parsed frontmatter, including scope and user-defined fields.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub meta: Map<String, Value>,
}

/// One loaded card: parsed frontmatter + body. The raw frontmatter mapping is
/// kept so writes can preserve keys this version does not know about.
#[derive(Debug, Clone)]
pub struct Card {
    /// Location within the deck (no `.md`).
    pub path: String,
    pub description: Option<String>,
    pub body: String,
    /// Revision of [`render`] — see [`revision_of`].
    pub revision: String,
    /// Raw frontmatter mapping (unknown keys preserved across rewrites).
    pub meta: Map<String, Value>,
}

impl Card {
    /// Frontmatter `id:` — the card's identity across moves. The model never
    /// sees it: [`view_of`] omits it.
    pub fn id(&self) -> Option<&str> {
        self.meta
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
    }

    /// The exact text handed to the model. [`revision_of`] hashes exactly
    /// this.
    pub fn render(&self) -> String {
        render(&self.path, &self.body)
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

    /// `pinned: true` — loaded into context when the deck is mounted.
    pub fn pinned(&self) -> bool {
        matches!(self.meta.get("pinned"), Some(Value::Bool(true)))
    }

    /// `link: <deck-id>:<path>` — a link card; its body is ignored. The
    /// model writes `@rumu:/cards/<path>` and rumu stores where it resolves.
    pub fn link(&self) -> Option<&str> {
        self.meta.get("link").and_then(Value::as_str)
    }

    /// The deck id and path a stored link points at.
    pub fn link_target(&self) -> Option<(&str, &str)> {
        self.link().and_then(|value| value.split_once(':'))
    }
}

/// A fresh UUIDv4 identity (deck ids).
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The structured header of a card `read`: where its content lives, kept out
/// of the editable view so read→write round trips are byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CardReadHeader {
    /// The deck holding the content (a link target's deck for a link).
    pub deck: String,
    /// The content's path within that deck.
    pub path: String,
}

impl Structured for CardReadHeader {
    const TAG: &'static str = "card.read";
    const V: u32 = 1;
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

/// `# {id}\n{body}` — the model-facing rendering of a card.
pub fn render(id: &str, body: &str) -> String {
    format!("# {id}\n{body}")
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

/// The editable view of a card (what `read` returns after its header and
/// `write`/`edit` accept for `workspace: card`): YAML frontmatter holding
/// `description` first, then every other key, followed by the body. The
/// frontmatter block is omitted when it would be empty.
pub fn view_of(card: &Card) -> String {
    let ystr = |s: &str| Value::String(s.to_owned());
    let mut m = Map::new();
    if let Some(d) = &card.description {
        m.insert("description".into(), ystr(d));
    }
    // Every other key rides along in its original order. `id` is identity,
    // not content: the model never sees or edits it.
    for (k, v) in &card.meta {
        if k == "description" || k == "id" {
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
/// passes). Errors name the offending field.
pub fn card_from_view(path: &str, view: &str) -> Result<Card, CardViewError> {
    let (yaml, body) = split_frontmatter(view);
    let mut meta = Map::new();
    let mut description = None;

    if let Some(yaml) = yaml {
        let yaml = quote_at_scalars(yaml);
        let parsed: Value = if yaml.trim().is_empty() {
            Value::Object(Map::new())
        } else {
            yaml::from_str(&yaml).map_err(CardViewError::InvalidYaml)?
        };
        let Value::Object(map) = parsed else {
            return Err(CardViewError::NotMapping);
        };
        for (key, v) in map {
            match key.as_str() {
                "description" => match v {
                    Value::Null => {}
                    Value::String(s) => description = Some(s),
                    _ => return Err(CardViewError::FieldNotString { field: key }),
                },
                // Unknown keys pass through.
                _ => {
                    meta.insert(key, v);
                }
            }
        }
    }

    // A link holds only its target: every other field and the body is
    // dropped, so nothing of its own can show anywhere.
    let body = if let Some(link) = meta.remove("link") {
        meta = Map::from_iter([("link".to_owned(), link)]);
        description = None;
        ""
    } else {
        body
    };
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
        path: path.to_owned(),
        description,
        body,
        revision: String::new(),
        meta,
    };
    card.revision = revision_of(&card.render());
    Ok(card)
}

/// Parse a loaded card file into a [`Card`] (computes the revision).
pub fn parse_card(path: &str, text: &str) -> Card {
    let (yaml, body) = split_frontmatter(text);
    let meta_mapping: Map<String, Value> = yaml
        .and_then(|y| yaml::from_str::<Value>(&quote_at_scalars(y)).ok())
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
    let revision = revision_of(&render(path, &body));
    Card {
        path: path.to_owned(),
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
    set(&mut m, "description", card.description.as_deref().map(ystr));

    let mut content = yaml_block(m);
    content.push_str(&card.body);
    if !card.body.ends_with('\n') {
        content.push('\n');
    }
    content
}

/// YAML treats a leading `@` as a reserved indicator, but the spec writes
/// link targets as `link: @rumu:/cards/<path>`. Quote that one scalar before
/// parsing so the model can use the documented spelling.
fn quote_at_scalars(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len() + 2);
    for line in yaml.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("link:") {
            let value = rest.trim();
            if value.starts_with('@') {
                let indent = &line[..line.len() - trimmed.len()];
                out.push_str(indent);
                out.push_str("link: ");
                out.push_str(&serde_json::to_string(value).unwrap_or_default());
                if line.ends_with('\n') {
                    out.push('\n');
                }
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Split leading `---\n…\n---\n` YAML frontmatter off a card file (or an
/// editable view).
pub fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, text);
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

/// Does the frontmatter of `text` fail to parse as a mapping?
pub fn frontmatter_is_broken(text: &str) -> bool {
    match split_frontmatter(text).0 {
        Some(yaml) if !yaml.trim().is_empty() => !matches!(
            yaml::from_str::<Value>(&quote_at_scalars(yaml)),
            Ok(Value::Object(_))
        ),
        _ => false,
    }
}

/// Card paths are `/`-separated segments within a deck. Each segment is
/// non-empty, must not start with `.` and uses `[A-Za-z0-9._-]` only — so `..`
/// and absolute paths are impossible, and `@` (the key/revision separator)
/// cannot appear. Max 200 chars, 8 levels deep.
pub fn valid_card_path(id: &str) -> bool {
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

/// A Markdown link in a card body, as written and as resolved against the
/// card: `Ok(deck-local path)` for a link relative to it (`./x`, `../d/y.md`),
/// `Err(())` for a mount path (`@rumu:/cards/…`, `/cards/…`) — those depend on
/// where the deck is mounted.
pub type BodyLink<'a> = (&'a str, Result<String, ()>);

/// The card-relative and mount-path links in `body` (fenced and inline code
/// skipped). External links, anchors and other workspaces are left out.
pub fn body_links<'a>(path: &str, body: &'a str) -> Vec<BodyLink<'a>> {
    let mut out = Vec::new();
    let mut fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        let mut code = false;
        let mut rest = line;
        while let Some(i) = rest.find(['`', ']']) {
            let tail = &rest[i + 1..];
            if rest.as_bytes()[i] == b'`' {
                code = !code;
                rest = tail;
                continue;
            }
            rest = tail;
            if code {
                continue;
            }
            let Some(inner) = tail.strip_prefix('(') else {
                continue;
            };
            let end = inner.find([')', ' ']).unwrap_or(inner.len());
            let href = &inner[..end];
            rest = &inner[end..];
            if let Some(link) = body_link(path, href) {
                out.push((href, link));
            }
        }
    }
    out
}

fn body_link(path: &str, href: &str) -> Option<Result<String, ()>> {
    let href = href.trim_matches(['<', '>']);
    if href.starts_with(crate::cards::LINK_PREFIX) || href.starts_with("/cards/") {
        return Some(Err(()));
    }
    let scheme = href.split_once(':').is_some_and(|(s, _)| !s.contains('/'));
    if href.is_empty() || href.starts_with(['/', '#']) || scheme {
        return None;
    }
    let href = href.split(['#', '?']).next().unwrap_or_default();
    let mut segs: Vec<&str> = path.split('/').collect();
    segs.pop();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                // Above the deck root resolves to "", which names no card.
                if segs.pop().is_none() {
                    return Some(Ok(String::new()));
                }
            }
            part => segs.push(part),
        }
    }
    let joined = segs.join("/");
    Some(Ok(joined.strip_suffix(".md").unwrap_or(&joined).to_owned()))
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

    #[test]
    fn body_links_resolve_against_the_card() {
        let body = "See [loop](./loop), [y](../x/y.md#h \"t\") and [up](../../../z).\n\
                    Mounted: [m](@rumu:/cards/self/m), [n](/cards/n).\n\
                    Not links: [w](https://x.io), [f](proj:src/a.rs), [a](#top), `[c](./c)`.\n\
                    ```\n[d](./d)\n```\n";
        let links = body_links("dev/rumu/kernel", body);
        assert_eq!(
            links,
            vec![
                ("./loop", Ok("dev/rumu/loop".to_owned())),
                ("../x/y.md#h", Ok("dev/x/y".to_owned())),
                ("../../../z", Ok(String::new())),
                ("@rumu:/cards/self/m", Err(())),
                ("/cards/n", Err(())),
            ]
        );
    }

    /// Card paths follow the path rule; malformed forms are rejected.
    #[test]
    fn paths() {
        for ok in [
            "a",
            "dev",
            "dev/rumu",
            "dev/rumu/kernel",
            "a.b",
            "A-b_9",
            "x.y/z",
        ] {
            assert!(valid_card_path(ok), "should accept `{ok}`");
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
            "@rumu:/cards/x",
            "日本語",
        ] {
            assert!(!valid_card_path(bad), "should reject `{bad}`");
        }
        assert!(!valid_card_path(&"a".repeat(201)), "too long");
        assert!(!valid_card_path(&["a"; 9].join("/")), "too deep");
        assert!(valid_card_path(&["a"; 8].join("/")), "depth 8 ok");
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
            "---\ndescription: d\ntags: [a, b]\nmatch:\n  event: prompt\nowner: test\n---\nbody\n",
        );
        assert_eq!(card.description.as_deref(), Some("d"));
        // `tags:` is an ordinary unknown key: kept verbatim.
        assert_eq!(card.meta.get("tags"), Some(&serde_json::json!(["a", "b"])));
        // Metadata-only changes do not move the revision.
        assert_eq!(card.render(), "# m\nbody\n");
        assert_eq!(card.revision, revision_of("# m\nbody\n"));
        let content = card_content(&card);
        assert!(content.contains("description: d"), "{content}");
        assert!(content.contains("owner: test"), "{content}");
        assert!(content.contains("tags"), "{content}");
        assert!(content.contains("match:"), "{content}");
        assert!(content.contains("event: prompt"), "{content}");
        assert!(content.ends_with("body\n"));

        // Re-parse: same known fields and revision.
        let again = parse_card("m", &content);
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
            "---\ndescription: d\ntags: [a, b]\nmatch:\n  event: prompt\nscope: [proj]\nowner: test\n---\nbody\n",
        );
        let view = view_of(&card);
        assert!(
            view.contains("description: d") && view.contains("owner: test"),
            "{view}"
        );
        assert!(view.contains("match:\n  event: prompt"), "{view}");
        assert!(view.contains("scope:"), "{view}");
        assert!(view.contains("tags"), "{view}");
        assert!(view.ends_with("---\nbody\n"), "{view}");

        // Parsing the view back keeps every frontmatter key; the written
        // file carries them unchanged.
        let back = card_from_view("m", &view).unwrap();
        assert_eq!(back.description.as_deref(), Some("d"));
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
        let err = card_from_view("x", "---\ndescription: 3\n---\nbody\n").unwrap_err();
        let err = err.to_string();
        assert!(err.contains("`description`"), "{err}");
        let err = card_from_view("x", "---\n- just\n- a\n- list\n---\nbody\n").unwrap_err();
        assert!(err.to_string().contains("mapping"), "{err}");
        // Null description clears it; unknown keys (tags included) pass through.
        let card =
            card_from_view("x", "---\ndescription: null\ntags: not-a-list\n---\nbody\n").unwrap();
        assert_eq!(card.description, None);
        assert_eq!(
            card.meta.get("tags"),
            Some(&Value::String("not-a-list".into()))
        );
        assert!(card_content(&card).contains("tags"));
        // No frontmatter at all: metadata-less card.
        let card = card_from_view("x", "body only\n").unwrap();
        assert_eq!(card.description, None);
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
