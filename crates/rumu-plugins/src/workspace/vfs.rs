//! The `@rumu` workspace's dispatch: one table of path patterns
//! ([`matchit`]), each served by a stateless [`Node`]. The file tools
//! resolve an `@rumu` path here and call the node's operation; whatever a
//! node does not serve is refused the same way everywhere.
//!
//! Paths name data. What can be done to one object beyond reading and
//! writing it is one of its node's actions ([`ActionSpec`]), run by
//! `invoke` with the action's full name (`<type>.<action>`, unique across
//! the table) and the object's path; `list` shows a node's actions.

use std::sync::LazyLock;

use async_trait::async_trait;
use matchit::{Params, Router};
use rumu_core::{Cx, PathState, RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult, Utf8UnixPath};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::WorkspacePlugin;
use super::cards::{Card, CardsDir};
use super::history::{CardHistory, HistoryRoot};
use super::registry::{Entry, ProjectDir, ProjectWorkspaces, WorkspacesDir};
use super::session::{ByAssistant, Session, Sessions};
use super::task::{Task, Tasks};
use super::tools::slice_lines;

/// What a node operation returns.
pub(super) type Out = ToolResult<ToolOutput>;

/// Every `@rumu` path. The roots come in the order `list @rumu:/` names
/// them; `/project` is last, as it exists only in a session with a project.
const ROUTES: &[(&str, &dyn Node)] = &[
    ("/", &Root),
    ("/cards", &CardsDir),
    ("/cards/{*path}", &Card),
    ("/history", &HistoryRoot),
    ("/history/cards/{*path}", &CardHistory),
    ("/sessions", &Sessions),
    ("/sessions/by-assistant", &ByAssistant),
    ("/sessions/by-assistant/{id}", &Sessions),
    ("/sessions/{sid}", &Session),
    ("/sessions/{sid}/{what}", &Session),
    ("/skills", &SkillFiles),
    ("/skills/{name}", &SkillFiles),
    ("/skills/{name}/{*rest}", &SkillFiles),
    ("/tasks", &Tasks),
    ("/tasks/{id}", &Task),
    ("/tools", &Tools),
    ("/tools/{server}", &Tools),
    ("/tools/{server}/{tool}", &Tools),
    ("/workspaces", &WorkspacesDir),
    ("/workspaces/{name}", &Entry),
    ("/project", &ProjectDir),
    ("/project/workspaces", &ProjectWorkspaces),
];

static ROUTER: LazyLock<Router<&'static dyn Node>> = LazyLock::new(|| {
    let mut router = Router::new();
    for (path, node) in ROUTES {
        router
            .insert(*path, *node)
            .expect("the @rumu routes are valid");
    }
    router
});

/// Every rumu action with the route it acts on. Building it checks that
/// each full name is `<type>.<action>` and unique (a node with actions is
/// registered under one route).
static ACTIONS: LazyLock<Vec<(&'static ActionSpec, &'static str)>> = LazyLock::new(|| {
    let mut all: Vec<(&'static ActionSpec, &'static str)> = Vec::new();
    for (path, node) in ROUTES {
        for spec in node.actions() {
            assert!(
                spec.name.contains('.') && !spec.name.contains('/'),
                "action `{}` is not <type>.<action>",
                spec.name
            );
            assert!(
                !all.iter().any(|(s, _)| s.name == spec.name),
                "action `{}` is declared twice",
                spec.name
            );
            all.push((spec, path));
        }
    }
    all
});

/// What an action on an object can do without asking: every action says.
#[derive(Debug, Clone, Copy)]
pub(super) enum Approval {
    /// Never asks the user (it touches only this branch or what it owns).
    Never,
    /// Asks for this deck action unless a grant covers it; the node's
    /// `invoke` puts the question through `approve`.
    #[expect(
        dead_code,
        reason = "`deck.mount` (the /decks step) is the first to ask"
    )]
    Asks(rumu_cards::deck::Action),
}

/// One action of a node: its full name, a line saying what it does, the
/// schema of its arguments, and whether it asks the user.
pub(super) struct ActionSpec {
    pub name: &'static str,
    pub about: &'static str,
    pub args: fn() -> Value,
    pub approval: Approval,
}

/// The arguments of an action that takes none.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct NoArgs {}

impl ActionSpec {
    /// Its arguments parsed, or an error with their schema.
    pub fn parse<T: DeserializeOwned>(&self, args: Value) -> ToolResult<T> {
        serde_json::from_value(args).map_err(|e| {
            ToolError::model(format!(
                "{}: invalid arguments: {e}; they are {}",
                self.name,
                (self.args)()
            ))
        })
    }

    /// Its line under `actions:`.
    fn line(&self) -> (String, String) {
        let mut about = self.about.to_owned();
        if let Approval::Asks(_) = self.approval {
            about.push_str(" (asks the user)");
        }
        (signature(self.name, &(self.args)()), about)
    }
}

/// `name(a: string, b?: integer)` from an object schema's properties.
pub(crate) fn signature(name: &str, schema: &Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let params: Vec<String> = schema["properties"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, prop)| {
            let opt = if required.contains(&key.as_str()) {
                ""
            } else {
                "?"
            };
            format!("{key}{opt}: {}", type_of(prop))
        })
        .collect();
    format!("{name}({})", params.join(", "))
}

/// A property's type, as short as a signature wants it.
fn type_of(prop: &Value) -> String {
    if let Some(values) = prop["enum"].as_array() {
        let names: Vec<String> = values.iter().map(Value::to_string).collect();
        return names.join(" | ");
    }
    let one = |t: &str| match t {
        "array" => format!("{}[]", type_of(&prop["items"])),
        "object" => "object".to_owned(),
        t => t.to_owned(),
    };
    match &prop["type"] {
        Value::String(t) => one(t),
        Value::Array(types) => {
            let names: Vec<String> = types
                .iter()
                .filter_map(Value::as_str)
                .filter(|t| *t != "null")
                .map(one)
                .collect();
            names.join(" | ")
        }
        _ => match prop["anyOf"].as_array().or(prop["oneOf"].as_array()) {
            Some(alts) => {
                let names: Vec<String> = alts
                    .iter()
                    .filter(|a| a["type"] != "null")
                    .map(type_of)
                    .collect();
                names.join(" | ")
            }
            None => "any".to_owned(),
        },
    }
}

/// The `actions:` section of a listing: `signature — what it does` lines.
pub(crate) fn actions_section(lines: impl IntoIterator<Item = (String, String)>) -> String {
    let mut out = String::from("actions:");
    for (sig, about) in lines {
        out.push_str(&format!("\n  {sig} — {about}"));
    }
    out
}

/// `/tasks/{id}` as the model writes it: `@rumu:/tasks/<id>`.
fn shown_route(pattern: &str) -> String {
    let path = pattern
        .replace('{', "<")
        .replace('}', ">")
        .replace("<*", "<");
    format!("{RUMU_WORKSPACE}:{path}")
}

/// Every rumu action and what it acts on, for refusals.
fn known_actions() -> String {
    let all: Vec<String> = ACTIONS
        .iter()
        .map(|(s, path)| format!("`{}` on `{}`", s.name, shown_route(path)))
        .collect();
    all.join(", ")
}

/// The refusal of `action`, which `node` (at `abs`) does not have.
pub(super) fn not_here(action: &str, abs: &Utf8UnixPath, node: &dyn Node) -> ToolError {
    let here = match node.actions() {
        [] => format!("`{RUMU_WORKSPACE}:{abs}` has no actions"),
        specs => {
            let names: Vec<String> = specs.iter().map(|s| format!("`{}`", s.name)).collect();
            format!("`{RUMU_WORKSPACE}:{abs}` has {}", names.join(", "))
        }
    };
    match ACTIONS.iter().find(|(s, _)| s.name == action) {
        Some((_, path)) => ToolError::model(format!(
            "`{action}` acts on `{}`; {here}",
            shown_route(path)
        )),
        None => unknown_action(action, &here),
    }
}

/// The refusal of a rumu action without a `path`.
pub(super) fn needs_path(action: &str) -> ToolError {
    match ACTIONS.iter().find(|(s, _)| s.name == action) {
        Some((_, path)) => ToolError::model(format!(
            "`{action}` needs `path`: the `{}` it acts on",
            shown_route(path)
        )),
        None => unknown_action(action, ""),
    }
}

fn unknown_action(action: &str, here: &str) -> ToolError {
    let here = if here.is_empty() {
        String::new()
    } else {
        format!("{here}; ")
    };
    ToolError::model(format!(
        "no action `{action}`; {here}rumu's actions: {} (`list` an object to see its own). An \
         MCP tool is `<server>/<tool>`.",
        known_actions()
    ))
}

/// The top-level directories, in table order.
static ROOTS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut roots: Vec<&str> = Vec::new();
    for (path, _) in ROUTES {
        let root = path.trim_start_matches('/').split('/').next().unwrap_or("");
        if !root.is_empty() && !roots.contains(&root) {
            roots.push(root);
        }
    }
    roots
});

/// One routed request: the plugin, the normalized path, its route
/// parameters, and the calling branch.
pub(super) struct At<'a> {
    pub p: &'a WorkspacePlugin,
    pub abs: &'a Utf8UnixPath,
    params: Params<'static, 'a>,
    pub state: &'a PathState,
    pub cx: &'a Cx,
}

impl At<'_> {
    /// A route parameter; empty when the matched route has none by that name.
    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).unwrap_or_default()
    }

    /// A route parameter, `None` when the matched route has none.
    pub fn opt(&self, name: &str) -> Option<&str> {
        self.params.get(name)
    }
}

/// What an `@rumu` path serves. Every node lists, and reads as its listing
/// unless it says otherwise; writing, deleting and searching are refused
/// unless the node serves them.
#[async_trait]
pub(super) trait Node: Send + Sync {
    async fn list(&self, at: &At<'_>) -> Out;

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let out = self.list(at).await?;
        slice_lines(out.content.text, offset, limit)
            .map(ToolOutput::ok)
            .map_err(ToolError::model)
    }

    /// A directory whose files are writable (refusals say so).
    fn holds_files(&self) -> bool {
        false
    }

    async fn write(&self, at: &At<'_>, _content: &str, _from: Option<&str>) -> Out {
        Err(self.refuse(at.abs, "write"))
    }

    async fn edit(&self, at: &At<'_>, _old: &str, _new: &str, _all: bool) -> Out {
        Err(self.refuse(at.abs, "edit"))
    }

    async fn delete(&self, at: &At<'_>) -> Out {
        if self.holds_files() {
            return Err(self.refuse(at.abs, "delete"));
        }
        Err(ToolError::model(format!(
            "`{RUMU_WORKSPACE}:{}` cannot be deleted",
            at.abs
        )))
    }

    async fn search(&self, _at: &At<'_>, _query: &str, _limit: Option<usize>) -> Out {
        Err(bad_scope())
    }

    /// What `invoke` can do to this object; `list` shows them.
    fn actions(&self) -> &'static [ActionSpec] {
        &[]
    }

    /// Run `spec`, one of [`Node::actions`] (the caller checked), with its
    /// raw arguments.
    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, _args: Value) -> Out {
        Err(ToolError::model(format!(
            "`{}` is not served at `{RUMU_WORKSPACE}:{}`",
            spec.name, at.abs
        )))
    }

    /// The refusal of a write, edit or delete here.
    fn refuse(&self, abs: &Utf8UnixPath, verb: &str) -> ToolError {
        if self.holds_files() {
            return ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{abs}` is a directory; {verb} a file under it"
            ));
        }
        let root = abs.as_str().trim_start_matches('/').split('/').next();
        ToolError::model(format!(
            "`{RUMU_WORKSPACE}:/{}` is read-only",
            root.unwrap_or_default()
        ))
    }
}

/// `list` of `node`: its own listing, then its actions.
pub(super) async fn list_node(node: &dyn Node, at: &At<'_>) -> Out {
    let mut out = node.list(at).await?;
    if !node.actions().is_empty() {
        let section = actions_section(node.actions().iter().map(ActionSpec::line));
        out.content.text = format!("{}\n\n{section}", out.content.text.trim_end());
    }
    Ok(out)
}

impl WorkspacePlugin {
    /// The node serving the `@rumu` path `abs`, with the request it gets.
    pub(super) fn route<'a>(
        &'a self,
        abs: &'a Utf8UnixPath,
        state: &'a PathState,
        cx: &'a Cx,
    ) -> ToolResult<(&'static dyn Node, At<'a>)> {
        let matched = ROUTER.at(abs.as_str()).map_err(|_| {
            let roots: Vec<String> = ROOTS.iter().map(|r| format!("/{r}")).collect();
            ToolError::model(format!(
                "unknown path `{RUMU_WORKSPACE}:{abs}` (roots: {})",
                roots.join(", ")
            ))
        })?;
        let params = matched.params;
        Ok((
            *matched.value,
            At {
                p: self,
                abs,
                params,
                state,
                cx,
            },
        ))
    }
}

/// The `search` scope refusal.
pub(super) fn bad_scope() -> ToolError {
    ToolError::model(format!(
        "`scope` must address the `{RUMU_WORKSPACE}` workspace: \
         `{RUMU_WORKSPACE}:/cards/<prefix>`, `{RUMU_WORKSPACE}:/sessions[/<session>]` or \
         `{RUMU_WORKSPACE}:/tools[/<server>]`"
    ))
}

/// `/`: the roots (`project/` only in a session with a project).
struct Root;

#[async_trait]
impl Node for Root {
    async fn list(&self, at: &At<'_>) -> Out {
        let roots: Vec<String> = ROOTS
            .iter()
            .filter(|r| **r != "project" || at.state.project.is_some())
            .map(|r| format!("{r}/"))
            .collect();
        Ok(roots.join("\n").into())
    }
}

/// `/tools[/<server>[/<tool>]]`: the session's MCP servers, one server's
/// tools, or one tool's description and input schema.
struct Tools;

#[async_trait]
impl Node for Tools {
    async fn list(&self, at: &At<'_>) -> Out {
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        match (at.opt("server"), at.opt("tool")) {
            (None, _) => Ok(crate::mcp::list_servers(&hub, &servers).await),
            (Some(server), None) => crate::mcp::list_tools(&hub, &servers, server).await,
            (Some(server), Some(tool)) => crate::mcp::read_tool(&hub, &servers, server, tool).await,
        }
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        if at.opt("tool").is_some() {
            return Err(bad_scope());
        }
        let limit = limit.unwrap_or(crate::mcp::SEARCH_LIMIT);
        let servers = crate::mcp::servers(at.p.decks(), at.state);
        crate::mcp::search(&at.p.mcp(), &servers, at.opt("server"), query, limit).await
    }
}

/// `/skills[/<name>[/<path>]]`: the session's skills, or a skill, or a file
/// or directory in one.
struct SkillFiles;

#[async_trait]
impl Node for SkillFiles {
    async fn list(&self, at: &At<'_>) -> Out {
        let (skills, rules) = (at.p.skills(), at.p.skill_rules(at.cx)?);
        match at.opt("name") {
            None => Ok(crate::skills::list_skills(&skills, &rules)),
            Some(name) => crate::skills::list(&skills, &rules, at.abs, name, at.param("rest")),
        }
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let (skills, rules) = (at.p.skills(), at.p.skill_rules(at.cx)?);
        let out = match at.opt("name") {
            None => crate::skills::list_skills(&skills, &rules),
            Some(name) => crate::skills::read(&skills, &rules, at.abs, name, at.param("rest"))?,
        };
        slice_lines(out.content.text, offset, limit)
            .map(ToolOutput::ok)
            .map_err(ToolError::model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_segments_win_over_parameters() {
        let params = |path: &str| -> String {
            let m = ROUTER.at(path).unwrap();
            let pairs: Vec<String> = m.params.iter().map(|(k, v)| format!("{k}={v}")).collect();
            pairs.join(" ")
        };
        // `by-assistant` is a static segment beside `{sid}`: it wins.
        assert_eq!(params("/sessions/by-assistant"), "");
        assert_eq!(params("/sessions/by-assistant/a1"), "id=a1");
        assert_eq!(params("/sessions/s1/a..b"), "sid=s1 what=a..b");
        assert_eq!(params("/cards/self/notes/x.md"), "path=self/notes/x.md");
        assert!(ROUTER.at("/sessions/s1/a/b").is_err());
        assert!(ROUTER.at("/history/cards").is_err());
        let roots = "cards history sessions skills tasks tools workspaces project";
        assert_eq!(ROOTS.join(" "), roots);
    }

    #[test]
    fn actions_have_unique_full_names_and_one_route_each() {
        let all: Vec<String> = ACTIONS
            .iter()
            .map(|(s, path)| format!("{} {path}", s.name))
            .collect();
        assert_eq!(
            all,
            ["task.kill /tasks/{id}", "workspace.use /workspaces/{name}"]
        );
        assert_eq!(shown_route("/cards/{*path}"), "@rumu:/cards/<path>");
    }

    #[test]
    fn signatures_name_each_argument_and_its_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "at": { "type": "string" },
                "write": { "type": ["boolean", "null"] },
                "tags": { "type": "array", "items": { "type": "string" } },
                "mode": { "enum": ["a", "b"] },
            },
            "required": ["at"],
        });
        assert_eq!(
            signature("deck.mount", &schema),
            r#"deck.mount(at: string, write?: boolean, tags?: string[], mode?: "a" | "b")"#
        );
        let none = rumu_core::tool_schema::<NoArgs>();
        assert_eq!(signature("task.kill", &none), "task.kill()");
    }
}
