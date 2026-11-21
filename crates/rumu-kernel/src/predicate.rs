//! Predicate engine: a hook `match` expression compiled once at load time
//! into a [`Pred`] tree, then evaluated against an event's normalized
//! [`Fields`].

use globset::{Glob, GlobMatcher};
use regex::Regex;
use rumu_core::{Fields, RUMU_WORKSPACE, Workspace, valid_workspace_name};
use serde_json::Value;

/// A matcher that failed validation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct PredicateError(String);

impl PredicateError {
    /// A validation message.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<String> for PredicateError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for PredicateError {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// A compiled matcher. [`Pred::compile`] is the only validation: unknown
/// fields and operators, wrong shapes and invalid regex/glob patterns all
/// fail there, so evaluation is infallible.
#[derive(Debug, Clone)]
pub enum Pred {
    /// AND of a matcher map's entries (`all:`).
    All(Vec<Pred>),
    /// OR — `any:` is the one way to say it.
    Any(Vec<Pred>),
    Not(Box<Pred>),
    /// One field: OR of conditions. List fields (`paths`) match
    /// when any element satisfies a whole condition.
    Field {
        field: Field,
        conds: Vec<Cond>,
    },
}

/// The matchable event fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Event,
    Tool,
    Cmd,
    Caller,
    Workspace,
    Text,
    Output,
    Paths,
    Source,
}

impl Field {
    fn parse(name: &str) -> Result<Self, String> {
        Ok(match name {
            "event" => Field::Event,
            "tool" => Field::Tool,
            "cmd" => Field::Cmd,
            "caller" => Field::Caller,
            "workspace" => Field::Workspace,
            "text" => Field::Text,
            "output" => Field::Output,
            "paths" => Field::Paths,
            "source" => Field::Source,
            other => return Err(format!("unknown field `{other}`")),
        })
    }

    /// The field's values: scalars yield zero or one, list fields every
    /// element. Missing (or empty) fields never match.
    fn values<'a>(&self, f: &'a Fields) -> Vec<&'a str> {
        match self {
            Field::Event => vec![f.event.as_str()],
            Field::Tool => f.tool.as_deref().into_iter().collect(),
            Field::Cmd => f.cmd.as_deref().into_iter().collect(),
            Field::Caller => f.caller.as_deref().into_iter().collect(),
            Field::Workspace => f.workspace.as_deref().into_iter().collect(),
            Field::Text => f.text.as_deref().into_iter().collect(),
            Field::Output => f.output.as_deref().into_iter().collect(),
            Field::Paths => f.paths.iter().map(String::as_str).collect(),
            Field::Source => f.source.as_deref().into_iter().collect(),
        }
    }
}

/// One condition on one field: AND of operators.
#[derive(Debug, Clone)]
pub struct Cond(Vec<Op>);

#[derive(Debug, Clone)]
enum Op {
    Eq(String),
    Has(Vec<String>),
    Glob(GlobMatcher),
    Re(Regex),
}

impl Pred {
    /// Compile a matcher (`HookSpec.matcher`, written as YAML by users).
    /// A predicate is a map (AND of entries); any other shape is an error.
    /// Workspace-qualified `paths` patterns are not expanded (there are no
    /// workspaces); use [`Pred::compile_with_workspaces`] to resolve them.
    pub fn compile(v: &Value) -> Result<Pred, PredicateError> {
        Self::compile_with_workspaces(v, &[])
    }

    /// Compile with the workspace registry, so `paths` patterns written
    /// as `<workspace>:<path>` expand to machine paths (`proj:web/**` →
    /// `<proj.root>/web/**`; `@rumu:/cards/**` stays rooted at `/`).
    pub fn compile_with_workspaces(
        v: &Value,
        workspaces: &[Workspace],
    ) -> Result<Pred, PredicateError> {
        Self::compile_inner(v, workspaces).map_err(PredicateError::from)
    }

    fn compile_inner(v: &Value, workspaces: &[Workspace]) -> Result<Pred, String> {
        let Value::Object(map) = v else {
            return Err(match v {
                Value::Array(_) => "a list is not a predicate — use `any: [ ... ]` for OR".into(),
                Value::String(s) => {
                    format!("a bare string is not a predicate — write `event: {s}`")
                }
                _ => "a predicate is a map (AND of entries)".into(),
            });
        };
        let mut all = Vec::new();
        for (k, cond) in map {
            all.push(match k.as_str() {
                "not" => Pred::Not(Box::new(
                    Pred::compile_inner(cond, workspaces).map_err(|e| format!("in `not`: {e}"))?,
                )),
                "any" | "all" => {
                    let Some(list) = cond.as_array() else {
                        return Err(format!("`{k}` expects a list of predicates"));
                    };
                    let preds = list
                        .iter()
                        .map(|p| {
                            Pred::compile_inner(p, workspaces).map_err(|e| format!("in `{k}`: {e}"))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if k == "any" {
                        Pred::Any(preds)
                    } else {
                        Pred::All(preds)
                    }
                }
                field => Pred::Field {
                    field: Field::parse(field)?,
                    conds: compile_conds(cond, field, workspaces)?,
                },
            });
        }
        Ok(Pred::All(all))
    }

    pub fn matches(&self, f: &Fields) -> bool {
        match self {
            Pred::All(ps) => ps.iter().all(|p| p.matches(f)),
            Pred::Any(ps) => ps.iter().any(|p| p.matches(f)),
            Pred::Not(p) => !p.matches(f),
            Pred::Field { field, conds } => field
                .values(f)
                .iter()
                .any(|v| conds.iter().any(|c| c.matches(v))),
        }
    }
}

/// A field condition: a plain string (= eq), a map of operators (AND), or a
/// list of those (OR).
fn compile_conds(v: &Value, field: &str, workspaces: &[Workspace]) -> Result<Vec<Cond>, String> {
    match v {
        Value::Array(list) => list
            .iter()
            .map(|c| Cond::compile(c, field, workspaces))
            .collect(),
        _ => Ok(vec![Cond::compile(v, field, workspaces)?]),
    }
}

impl Cond {
    fn compile(v: &Value, field: &str, workspaces: &[Workspace]) -> Result<Cond, String> {
        let path_cond = field == "paths";
        let ops = match v {
            Value::String(s) => {
                let s = if path_cond {
                    expand_path_pattern(s, workspaces)?
                } else {
                    s.clone()
                };
                vec![Op::Eq(if path_cond {
                    expand_home(&s, home().as_deref())
                } else {
                    s
                })]
            }
            Value::Object(map) => {
                let mut ops = Vec::new();
                for (op, arg) in map {
                    ops.push(match op.as_str() {
                        "eq" => {
                            let s = arg.as_str().ok_or("`eq` expects a string")?.to_owned();
                            let s = if path_cond {
                                expand_path_pattern(&s, workspaces)?
                            } else {
                                s
                            };
                            Op::Eq(if path_cond {
                                expand_home(&s, home().as_deref())
                            } else {
                                s
                            })
                        }
                        "has" => Op::Has(has_keywords(arg)?),
                        "glob" => {
                            let p = arg.as_str().ok_or("`glob` expects a string pattern")?;
                            let p = if path_cond {
                                expand_path_pattern(p, workspaces)?
                            } else {
                                p.to_owned()
                            };
                            Op::Glob(compile_glob(&p)?)
                        }
                        "re" => Op::Re(compile_re(arg)?),
                        other => {
                            return Err(format!("unknown operator `{other}` (eq, has, glob, re)"));
                        }
                    });
                }
                ops
            }
            _ => {
                return Err(format!(
                    "field `{field}` expects a string, an operator map, or a list of those"
                ));
            }
        };
        Ok(Cond(ops))
    }

    fn matches(&self, v: &str) -> bool {
        self.0.iter().all(|op| op.matches(v))
    }
}

impl Op {
    fn matches(&self, v: &str) -> bool {
        match self {
            Op::Eq(s) => v == s,
            Op::Has(kws) => kws.iter().any(|k| has_match(v, k)),
            Op::Glob(g) => g.is_match(v),
            Op::Re(re) => re.is_match(v),
        }
    }
}

/// `has` operand: a keyword or a list of keywords (any of them matches).
fn has_keywords(arg: &Value) -> Result<Vec<String>, String> {
    let keyword = |v: &Value| {
        v.as_str()
            .map(str::to_owned)
            .ok_or_else(|| "`has` expects a string or a list of strings".to_owned())
    };
    match arg {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(list) => list.iter().map(keyword).collect(),
        _ => Err("`has` expects a string or a list of strings".into()),
    }
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, String> {
    let pattern = expand_home(pattern, home().as_deref());
    Glob::new(&pattern)
        .map_err(|e| format!("invalid glob `{pattern}`: {e}"))
        .map(|g| g.compile_matcher())
}

fn compile_re(arg: &Value) -> Result<Regex, String> {
    let p = arg.as_str().ok_or("`re` expects a string pattern")?;
    Regex::new(p).map_err(|e| format!("invalid regex `{p}`: {e}"))
}

/// Case-insensitive keyword containment. Pure-ASCII keywords require word
/// boundaries (`rl` must not hit `url`); keywords containing non-ASCII (CJK)
/// use plain substring match.
fn has_match(text: &str, keyword: &str) -> bool {
    if keyword.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let kw = keyword.to_lowercase();
    if !keyword.is_ascii() {
        return text.contains(&kw);
    }
    text.match_indices(kw.as_str()).any(|(i, matched)| {
        let before_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after = &text[i + matched.len()..];
        let after_ok = after.chars().next().is_none_or(|c| !is_word_char(c));
        before_ok && after_ok
    })
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

/// Expand a `<workspace>:<path>` prefix in a `paths` condition to a
/// machine-absolute pattern:
///
/// - `@rumu:<rest>` / `@rumu:/<rest>` → `/<rest>` (a `/cards/…` path without
///   glob characters also loses a trailing `.md`);
/// - `<preset>:<rest>` → `<preset.root>/<rest>` (or `<rest>` when absolute);
/// - a valid workspace name that is not a preset is a compile error;
/// - anything else (machine paths, reserved scheme-looking names) is left
///   untouched.
fn expand_path_pattern(s: &str, workspaces: &[Workspace]) -> Result<String, String> {
    let Some((name, rest)) = s.split_once(':') else {
        return Ok(s.to_owned());
    };
    if name == RUMU_WORKSPACE {
        let path = format!("/{}", rest.trim_start_matches('/'));
        let globbed = path.chars().any(|c| matches!(c, '*' | '?' | '[' | '{'));
        if !globbed && path.starts_with("/cards/") {
            return Ok(path.strip_suffix(".md").unwrap_or(&path).to_owned());
        }
        return Ok(path);
    }
    if !valid_workspace_name(name) {
        return Ok(s.to_owned());
    }
    let ws = workspaces
        .iter()
        .find(|w| w.name == name)
        .ok_or_else(|| format!("unknown workspace `{name}` in `paths` pattern"))?;
    if rest.starts_with('/') {
        return Ok(rest.to_owned());
    }
    Ok(format!(
        "{}/{}",
        ws.root.as_str().trim_end_matches('/'),
        rest
    ))
}

/// `~` / `~/` → `home`.
fn expand_home(s: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return s.to_owned();
    };
    if s == "~" {
        return home.to_owned();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    s.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields() -> Fields {
        Fields {
            event: "tool.pre".into(),
            tool: Some("bash".into()),
            cmd: Some("rm -rf /tmp/x".into()),
            caller: Some("model".into()),
            source: None,
            workspace: Some("proj".into()),
            paths: vec!["/home/u/code/proj/Cargo.toml".into()],
            text: Some("check the url and config".into()),
            output: None,
        }
    }

    fn m(v: Value) -> Pred {
        Pred::compile(&v).unwrap()
    }

    #[test]
    fn field_operators() {
        let f = fields();

        // Equality and missing fields.
        assert!(m(json!({"event": "tool.pre", "tool": "bash"})).matches(&f));
        assert!(!m(json!({"tool": "fs"})).matches(&f));
        assert!(!m(json!({"output": {"has": "x"}})).matches(&f));

        // The workspace field: plain string = eq, like event/tool.
        assert!(m(json!({"workspace": "proj"})).matches(&f));
        assert!(m(json!({"workspace": {"eq": "proj"}})).matches(&f));
        assert!(!m(json!({"workspace": "@rumu"})).matches(&f));
        let no_ws = Fields {
            workspace: None,
            ..fields()
        };
        assert!(!m(json!({"workspace": "proj"})).matches(&no_ws));

        // Regexes.
        assert!(m(json!({"cmd": {"re": "^rm -rf /"}})).matches(&f));
        assert!(!m(json!({"cmd": {"re": "^rm -rf /etc"}})).matches(&f));

        // Boolean combinators.
        assert!(m(json!({"not": {"tool": "fs"}})).matches(&f));
        assert!(m(json!({"any": [{"tool": "fs"}, {"cmd": {"re": "rm -rf"}}]})).matches(&f));
        assert!(!m(json!({"all": [{"tool": "fs"}, {"cmd": {"re": "rm -rf"}}]})).matches(&f));

        // List fields match when any element matches; a field-condition
        // list is OR.
        assert!(m(json!({"paths": {"eq": "/home/u/code/proj/Cargo.toml"}})).matches(&f));
        assert!(m(json!({"paths": {"has": "cargo"}})).matches(&f));
        assert!(
            m(json!({"paths": [{"eq": "/cards/x"}, {"glob": "/home/u/code/proj/**"}]})).matches(&f)
        );

        // An empty list never matches.
        let empty_paths = Fields {
            paths: vec![],
            ..fields()
        };
        assert!(!m(json!({"paths": {"glob": "**"}})).matches(&empty_paths));
    }

    #[test]
    fn text_predicates() {
        let mut f = Fields {
            event: "prompt".into(),
            ..fields()
        };

        // `has` uses word boundaries for ASCII: "rl" must not hit "url".
        assert!(!m(json!({"text": {"has": "rl"}})).matches(&f));
        f.text = Some("the rl service is up".into());
        assert!(m(json!({"text": {"has": "rl"}})).matches(&f));
        f.text = Some("RL service".into());
        assert!(m(json!({"text": {"has": "rl"}})).matches(&f)); // case-insensitive

        // CJK keywords match as substrings.
        f.text = Some("帮我看下代理服务器配置".into());
        assert!(m(json!({"text": {"has": "代理"}})).matches(&f));
        f.text = Some("反向代理".into());
        assert!(m(json!({"text": {"has": "代理"}})).matches(&f));

        // Inline keyword lists are OR.
        f.text = Some("deploy the k8s cluster".into());
        assert!(m(json!({"text": {"has": ["gateway", "k8s"]}})).matches(&f));
        f.text = Some("nothing here".into());
        assert!(!m(json!({"text": {"has": ["gateway", "k8s"]}})).matches(&f));
    }

    #[test]
    fn glob_with_home() {
        let f = fields();
        // `~` expansion is exercised directly (compile reads $HOME).
        let h = Some("/home/u");
        assert_eq!(expand_home("~", h), "/home/u");
        assert_eq!(expand_home("~/code/**", h), "/home/u/code/**");
        assert_eq!(expand_home("/srv", h), "/srv");
        // engine-level globs without `~`
        assert!(m(json!({"workspace": {"glob": "p*"}})).matches(&f));
        assert!(m(json!({"paths": {"glob": "**/Cargo.toml"}})).matches(&f));
        assert!(!m(json!({"workspace": {"glob": "nowhere*"}})).matches(&f));
    }

    #[test]
    fn paths_workspace_patterns_expand_with_the_registry() {
        let workspaces = [Workspace {
            name: "proj".into(),
            ssh: None,
            root: "/home/u/code/proj".into(),
            read_only: false,
        }];
        let compile = |v: Value| Pred::compile_with_workspaces(&v, &workspaces);

        let proj = fields(); // paths: /home/u/code/proj/Cargo.toml
        // `proj:web/**` joins the preset root; `proj:/abs` stays absolute.
        assert!(
            compile(json!({"paths": {"glob": "proj:Cargo.toml"}}))
                .unwrap()
                .matches(&proj)
        );
        assert!(
            compile(json!({"paths": {"glob": "proj:/home/u/code/proj/Cargo.*"}}))
                .unwrap()
                .matches(&proj)
        );
        assert!(
            !compile(json!({"paths": {"glob": "proj:src/**"}}))
                .unwrap()
                .matches(&proj)
        );

        // The `@rumu` virtual workspace: all three spellings are equivalent.
        let rumu_paths = Fields {
            paths: vec!["/cards/dev/rumu".into()],
            ..fields()
        };
        for pattern in ["@rumu:/cards/**", "@rumu:cards/**", "/cards/**"] {
            assert!(
                compile(json!({ "paths": { "glob": pattern } }))
                    .unwrap()
                    .matches(&rumu_paths),
                "{pattern}"
            );
        }
        // A card path without globs loses a trailing `.md`; `eq` and the bare
        // string form share the expansion.
        for cond in [
            json!({ "eq": "@rumu:/cards/dev/rumu.md" }),
            json!("@rumu:/cards/dev/rumu"),
        ] {
            assert!(
                compile(json!({ "paths": cond }))
                    .unwrap()
                    .matches(&rumu_paths)
            );
        }

        // A valid-looking workspace name that is not a preset fails to compile.
        let err = compile(json!({"paths": {"glob": "ghost:x/**"}})).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown workspace `ghost` in `paths` pattern"
        );

        // Reserved scheme names and plain machine paths pass through untouched.
        assert!(compile(json!({"paths": "/cards/**"})).is_ok());
        assert!(compile(json!({"paths": "https://example.com/x"})).is_ok());
    }

    #[test]
    fn compile_rejects_bad_matchers() {
        // Lists at predicate position point at `any:` (the one way to say
        // OR), recursively through `not` / `any` / `all`; bare strings and
        // bools are rejected; unknown fields/operators, wrong shapes
        // and bad patterns all fail with a short message.
        for (v, needle) in [
            (json!([{"tool": "bash"}]), "any"),
            (json!({"not": [{"tool": "bash"}]}), "any"),
            (json!({"any": [{"tool": "bash"}, [{"cmd": "x"}]]}), "any"),
            (json!("prompt"), "bare string"),
            (json!(true), "map"),
            (json!({"toolz": "bash"}), "unknown field"),
            (json!({"cmd": {"rex": "x"}}), "unknown operator"),
            (json!({"cmd": {"eq": 3}}), "expects a string"),
            (json!({"cmd": {"re": "("}}), "invalid regex"),
            (json!({"cmd": 3}), "operator map"),
            (json!({"text": {"has": [1]}}), "list of strings"),
            (json!({"any": {"tool": "bash"}}), "list of predicates"),
        ] {
            let err = Pred::compile(&v).unwrap_err();
            let err = err.to_string();
            assert!(err.contains(needle), "{v}: {err}");
        }
        // Field-condition lists (OR) stay valid.
        assert!(Pred::compile(&json!({"paths": [{"glob": "a"}, {"glob": "b"}]})).is_ok());
    }
}
