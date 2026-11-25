//! The `@rumu` workspace's dispatch: one table of path patterns
//! ([`matchit`]), each served by a stateless [`Node`]. The file tools
//! resolve an `@rumu` path here and call the node's operation; whatever a
//! node does not serve is refused the same way everywhere.
//!
//! Paths name data. What can be done to one object beyond reading and
//! writing it is one of its node's actions ([`ActionSpec`]), run by
//! `invoke` with the action's full name (`<type>.<action>`, unique across
//! the table) and the object's path.
//!
//! Every node documents itself, so the system prompt need not: `list` of a
//! path shows its node's [`Node::doc`], then its entries, then the files
//! rumu defines under it (`files:`, each with its node's [`Node::about`]),
//! then its actions (`actions:`).
//!
//! A path under a top-level directory an external plugin mounts
//! (`@rumu:/<root>/...`) misses the table and is served by [`PluginNode`]:
//! every operation is the plugin's `vfs/*` request (rumu-core
//! [`PluginMount`]), and its listings are laid out like the others.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use matchit::{Params, Router};
use rumu_core::{
    Cx, MountListing, PathState, PluginMount, RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult,
    Utf8UnixPath,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{StdioError, StdioPlugin};

use super::WorkspacePlugin;
use super::cards::{Card, CardsDir};
use super::decks::{DeckFile, DecksDir};
use super::registry::{Entry, WorkspacesDir};
use super::session::{
    AssistantSessions, BranchNode, Branches, ByAssistant, Session, SessionPart, Sessions,
};
use super::task::{Task, TaskOutput, Tasks};
use super::tools::{replace_occurrences, slice_lines};

/// What a node operation returns.
pub(super) type Out = ToolResult<ToolOutput>;

/// Every `@rumu` path. The roots come in the order `list @rumu:/` names
/// them; a route with a fixed last segment under another is one of its
/// `files:`.
const ROUTES: &[(&str, &dyn Node)] = &[
    ("/", &Root),
    ("/cards", &CardsDir),
    ("/cards/{*path}", &Card),
    ("/decks", &DecksDir),
    ("/decks/{name}", &DeckFile),
    ("/sessions", &Sessions),
    ("/sessions/by-assistant", &ByAssistant),
    ("/sessions/by-assistant/{id}", &AssistantSessions),
    ("/sessions/{sid}", &Session),
    ("/sessions/{sid}/{what}", &SessionPart),
    ("/sessions/{sid}/branches", &Branches),
    ("/sessions/{sid}/branches/{bid}", &BranchNode),
    ("/skills", &SkillFiles),
    ("/skills/{name}", &SkillFiles),
    ("/skills/{name}/{*rest}", &SkillFiles),
    ("/tasks", &Tasks),
    ("/tasks/{id}", &Task),
    ("/tasks/{id}/output", &TaskOutput),
    ("/mcp", &Mcp),
    ("/mcp/{server}", &McpServer),
    ("/workspaces", &WorkspacesDir),
    ("/workspaces/{name}", &Entry),
];

/// The routes, each with its pattern (a listing finds its files by it).
static ROUTER: LazyLock<Router<(&'static str, &'static dyn Node)>> = LazyLock::new(|| {
    let mut router = Router::new();
    for (path, node) in ROUTES {
        router
            .insert(*path, (*path, *node))
            .expect("the @rumu routes are valid");
    }
    router
});

/// The files rumu defines right under `pattern`: routes one fixed segment
/// below it, as `name` (`name/` when routes lie below it too) and the
/// line its node says about itself.
fn files_of(pattern: &str) -> Vec<(String, &'static str)> {
    let prefix = format!("{}/", pattern.trim_end_matches('/'));
    ROUTES
        .iter()
        .filter_map(|(path, node)| {
            let name = path.strip_prefix(&prefix)?;
            if name.is_empty() || name.contains(['/', '{']) {
                return None;
            }
            let below = format!("{path}/");
            let dir = ROUTES.iter().any(|(p, _)| p.starts_with(&below));
            let shown = if dir {
                format!("{name}/")
            } else {
                name.to_owned()
            };
            Some((shown, node.about()))
        })
        .collect()
}

/// A listing section: its title, then `  name — about` lines.
fn section(title: &str, lines: impl IntoIterator<Item = (String, String)>) -> String {
    let mut out = title.to_owned();
    for (name, about) in lines {
        out.push_str(&format!("\n  {name} — {about}"));
    }
    out
}

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
        if let Approval::Asks(action) = self.approval {
            let action = format!("{action:?}").to_lowercase();
            about.push_str(&format!(
                " (asks the user: `{action}`, unless a grant covers it)"
            ));
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
    section("actions:", lines)
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
    // `task.list` and the like: what lists things is the `list` tool.
    let listing = action
        .strip_suffix(".list")
        .and_then(|kind| {
            Some(match kind {
                "task" => "/tasks",
                "session" => "/sessions",
                "branch" => "/sessions/<sid>/branches",
                "deck" => "/decks",
                "card" => "/cards",
                "workspace" => "/workspaces",
                "skill" => "/skills",
                "mcp" => "/mcp",
                _ => return None,
            })
        })
        .map(|path| format!("to list them, `list {RUMU_WORKSPACE}:{path}`; "))
        .unwrap_or_default();
    ToolError::model(format!(
        "no action `{action}`; {listing}{here}rumu's actions: {} (`list` an object to see its \
         own). An MCP tool is `<server>/<tool>`.",
        known_actions()
    ))
}

/// rumu's own top-level directories (a plugin may not mount one).
pub(crate) fn builtin_roots() -> &'static [&'static str] {
    &ROOTS
}

/// What a path under a plugin's mount matches: its root, and the rest.
static MOUNTED: LazyLock<Router<()>> = LazyLock::new(|| {
    let mut router = Router::new();
    router.insert("/{root}", ()).expect("valid route");
    router.insert("/{root}/{*rest}", ()).expect("valid route");
    router
});

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

/// One routed request: the plugin, the normalized path, the route it
/// matched and its parameters, and the calling branch.
pub(super) struct At<'a> {
    pub p: &'a WorkspacePlugin,
    pub abs: &'a Utf8UnixPath,
    pattern: &'static str,
    params: Params<'static, 'a>,
    pub state: &'a PathState,
    pub cx: &'a Cx,
    /// The mount and its plugin, for a path under a plugin's mount.
    mount: Option<(PluginMount, Arc<StdioPlugin>)>,
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

/// What an `@rumu` path serves. Every node says what it is and lists, and
/// reads as its listing unless it says otherwise; writing, deleting and
/// searching are refused unless the node serves them.
#[async_trait]
pub(super) trait Node: Send + Sync {
    /// One line on what it is: shown after its name where it is listed.
    fn about(&self) -> &'static str;

    /// What `list` shows first: what it holds, what its entries look like,
    /// what may be written and what asks the user. Empty when its entries
    /// say enough.
    fn doc(&self, _at: &At<'_>) -> &'static str {
        ""
    }

    /// Its entries (or, for a file, what it is now).
    async fn list(&self, at: &At<'_>) -> Out;

    /// The whole listing: its doc, its entries, the files rumu defines under
    /// it, and its actions; the parts it has, a blank line apart (just its
    /// entries, untouched, when it has nothing else).
    async fn listing(&self, at: &At<'_>) -> Out {
        let mut out = self.list(at).await?;
        let doc = self.doc(at).trim();
        let files = if at.pattern == "/" {
            Vec::new()
        } else {
            files_of(at.pattern)
        };
        if doc.is_empty() && files.is_empty() && self.actions().is_empty() {
            return Ok(out);
        }
        let mut parts: Vec<String> = Vec::new();
        if !doc.is_empty() {
            parts.push(doc.to_owned());
        }
        let entries = out.content.text.trim_end();
        if !entries.is_empty() {
            parts.push(entries.to_owned());
        }
        if !files.is_empty() {
            let lines = files.into_iter().map(|(name, about)| (name, about.into()));
            parts.push(section("files:", lines));
        }
        if !self.actions().is_empty() {
            parts.push(actions_section(self.actions().iter().map(ActionSpec::line)));
        }
        out.content.text = parts.join("\n\n");
        Ok(out)
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let out = self.listing(at).await?;
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

impl WorkspacePlugin {
    /// The node serving the `@rumu` path `abs`, with the request it gets.
    pub(super) fn route<'a>(
        &'a self,
        abs: &'a Utf8UnixPath,
        state: &'a PathState,
        cx: &'a Cx,
    ) -> ToolResult<(&'static dyn Node, At<'a>)> {
        let Ok(matched) = ROUTER.at(abs.as_str()) else {
            return self.route_mounted(abs, state, cx);
        };
        let params = matched.params;
        let (pattern, node) = *matched.value;
        Ok((
            node,
            At {
                p: self,
                abs,
                pattern,
                params,
                state,
                cx,
                mount: None,
            },
        ))
    }

    /// A path no rumu route serves: one under a plugin's mount, or unknown.
    fn route_mounted<'a>(
        &'a self,
        abs: &'a Utf8UnixPath,
        state: &'a PathState,
        cx: &'a Cx,
    ) -> ToolResult<(&'static dyn Node, At<'a>)> {
        let mounts = self.mounts();
        let unknown = || {
            let mut roots: Vec<String> = ROOTS.iter().map(|r| format!("/{r}")).collect();
            roots.extend(mounts.iter().map(|(m, _)| format!("/{}", m.root)));
            ToolError::model(format!(
                "unknown path `{RUMU_WORKSPACE}:{abs}` (roots: {})",
                roots.join(", ")
            ))
        };
        let matched = MOUNTED.at(abs.as_str()).map_err(|_| unknown())?;
        let root = matched.params.get("root").unwrap_or_default();
        let Some(mount) = mounts.iter().find(|(m, _)| m.root == root).cloned() else {
            return Err(unknown());
        };
        Ok((
            &PluginNode,
            At {
                p: self,
                abs,
                pattern: "/{root}/{*rest}",
                params: matched.params,
                state,
                cx,
                mount: Some(mount),
            },
        ))
    }
}

/// The `search` scope refusal.
pub(super) fn bad_scope() -> ToolError {
    ToolError::model(format!(
        "`scope` must address the `{RUMU_WORKSPACE}` workspace: \
         `{RUMU_WORKSPACE}:/cards/<prefix>`, `{RUMU_WORKSPACE}:/sessions[/<session>]` or \
         `{RUMU_WORKSPACE}:/mcp[/<server>]`"
    ))
}

/// `/`: the roots.
struct Root;

#[async_trait]
impl Node for Root {
    fn about(&self) -> &'static str {
        "rumu's own data"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "rumu's own data, as paths (no `bash`). `list` any of them for what it holds, what may \
         be written and what asks the user, and the actions `invoke` runs on it (`action` its \
         full name, `path` the object)."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let mut lines: Vec<String> = files_of("/")
            .into_iter()
            .map(|(name, about)| format!("{name} — {about}"))
            .collect();
        let mounts = at.p.mounts();
        lines.extend(mounts.iter().map(|(m, p)| {
            use rumu_core::Plugin;
            let plugin = p.manifest().name;
            format!("{}/ — {} (plugin `{plugin}`)", m.root, m.about)
        }));
        Ok(lines.join("\n").into())
    }
}

/// `/<root>[/<path>]` under a plugin's mount: each operation is the
/// plugin's `vfs/*` request with the path below the root. `edit` is a read
/// and a write; a `read` the plugin answers with `null` reads as the
/// listing, as everywhere in `@rumu`.
struct PluginNode;

impl PluginNode {
    /// The path below the mount's root (`/` for the root itself).
    fn below(at: &At<'_>) -> String {
        format!("/{}", at.param("rest"))
    }

    /// Ask the plugin `vfs/<op>` with `params` (an object) and the path.
    async fn ask(at: &At<'_>, op: &str, mut params: Value) -> ToolResult<Value> {
        let (mount, plugin) = at.mount.as_ref().expect("a mounted path has its plugin");
        params["path"] = json!(Self::below(at));
        plugin.vfs(op, params, at.cx).await.map_err(|e| match e {
            StdioError::Remote { message, .. } => ToolError::model(message),
            e => ToolError::model(format!("`{RUMU_WORKSPACE}:/{}`: {e}", mount.root)),
        })
    }

    /// A plugin's answer as a tool output: a `ToolOutput`, or bare text.
    fn output(at: &At<'_>, value: Value) -> Out {
        if let Value::String(text) = value {
            return Ok(ToolOutput::ok(text));
        }
        serde_json::from_value(value).map_err(|e| {
            ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{}`: the plugin answered no tool output: {e}",
                at.abs
            ))
        })
    }

    /// `from` (a `write` that moves) as a path below the same root.
    fn from_below(at: &At<'_>, from: &str) -> ToolResult<String> {
        let (mount, _) = at.mount.as_ref().expect("a mounted path has its plugin");
        let path = from
            .strip_prefix(RUMU_WORKSPACE)
            .and_then(|p| p.strip_prefix(':'))
            .unwrap_or(from);
        let root = format!("/{}", mount.root);
        match path.strip_prefix(&root) {
            Some("") => Ok("/".into()),
            Some(rest) if rest.starts_with('/') => Ok(rest.to_owned()),
            _ => Err(ToolError::model(format!(
                "`from` must be a path under `{RUMU_WORKSPACE}:{root}`"
            ))),
        }
    }

    /// The text a read gave, for an edit.
    async fn text_of(at: &At<'_>) -> ToolResult<String> {
        let read = Self::ask(at, "read", json!({})).await?;
        if read.is_null() {
            return Err(ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{}` is a directory; edit a file under it",
                at.abs
            )));
        }
        let out = Self::output(at, read)?;
        if out.is_error {
            return Err(ToolError::model(out.content.text));
        }
        if !out.content.attachments.is_empty() {
            return Err(ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{}` is not text",
                at.abs
            )));
        }
        Ok(out.content.text)
    }
}

/// `invoke` of `action` on a path under a plugin's mount (the plugin says
/// which actions it has).
pub(super) async fn invoke_mounted(at: &At<'_>, action: &str, args: Value) -> Option<Out> {
    at.mount.as_ref()?;
    let answer = PluginNode::ask(at, "invoke", json!({ "action": action, "args": args })).await;
    Some(answer.and_then(|v| PluginNode::output(at, v)))
}

#[async_trait]
impl Node for PluginNode {
    fn about(&self) -> &'static str {
        "a directory a plugin serves"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        self.listing(at).await
    }

    async fn listing(&self, at: &At<'_>) -> Out {
        let answer = Self::ask(at, "list", json!({})).await?;
        let listing: MountListing = serde_json::from_value(answer).map_err(|e| {
            ToolError::model(format!(
                "`{RUMU_WORKSPACE}:{}`: the plugin answered no listing: {e}",
                at.abs
            ))
        })?;
        let mut parts: Vec<String> = Vec::new();
        if let Some(doc) = listing
            .doc
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            parts.push(doc.to_owned());
        }
        let entries: Vec<String> = listing
            .entries
            .iter()
            .map(|e| {
                let slash = if e.dir == Some(true) { "/" } else { "" };
                match e.about.as_deref().filter(|a| !a.is_empty()) {
                    Some(about) => format!("{}{slash} — {about}", e.name),
                    None => format!("{}{slash}", e.name),
                }
            })
            .collect();
        if !entries.is_empty() {
            parts.push(entries.join("\n"));
        }
        if !listing.actions.is_empty() {
            let none = json!({ "type": "object", "properties": {} });
            let lines = listing.actions.iter().map(|a| {
                let args = a.args.as_ref().unwrap_or(&none);
                (signature(&a.name, args), a.about.clone())
            });
            parts.push(actions_section(lines));
        }
        Ok(parts.join("\n\n").into())
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let answer = Self::ask(at, "read", json!({})).await?;
        let mut out = if answer.is_null() {
            self.listing(at).await?
        } else {
            Self::output(at, answer)?
        };
        if offset.is_some() || limit.is_some() {
            out.content.text = slice_lines(std::mem::take(&mut out.content.text), offset, limit)
                .map_err(ToolError::model)?;
        }
        Ok(out)
    }

    async fn write(&self, at: &At<'_>, content: &str, from: Option<&str>) -> Out {
        let from = from.map(|f| Self::from_below(at, f)).transpose()?;
        let answer = Self::ask(at, "write", json!({ "content": content, "from": from })).await?;
        Self::output(at, answer)
    }

    async fn edit(&self, at: &At<'_>, old: &str, new: &str, all: bool) -> Out {
        let text = Self::text_of(at).await?;
        let (text, n) = replace_occurrences(&text, old, new, all).map_err(ToolError::model)?;
        let answer = Self::ask(at, "write", json!({ "content": text, "from": null })).await?;
        let mut out = Self::output(at, answer)?;
        if !out.is_error {
            let s = if n == 1 { "" } else { "s" };
            out.content.text = format!("edited `{RUMU_WORKSPACE}:{}` ({n} replacement{s})", at.abs);
        }
        Ok(out)
    }

    async fn delete(&self, at: &At<'_>) -> Out {
        let answer = Self::ask(at, "delete", json!({})).await?;
        Self::output(at, answer)
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        let answer = Self::ask(at, "search", json!({ "query": query, "limit": limit })).await?;
        Self::output(at, answer)
    }
}

/// `/mcp`: the session's MCP servers.
struct Mcp;

#[async_trait]
impl Node for Mcp {
    fn about(&self) -> &'static str {
        "this session's MCP servers; a server's tools are its actions"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "This session's MCP servers. A server's tools are its actions: `invoke` one with \
         `action: \"<server>/<tool>\"` and no `path`. `list @rumu:/mcp/<server>` shows them and \
         the server's resources; `read` it for its instructions and each tool's input schema. \
         `search` with this scope (or a server's) finds tools. Each server is one connection \
         every session shares: `invoke` `mcp.spawn` on a server for one of this session's own \
         (an instance, `<server>#<id>`, used like its server); `delete` an instance to close it."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        Ok(crate::mcp::list_servers(&hub, &servers).await)
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        McpServer.search(at, query, limit).await
    }
}

/// `/mcp/<server>`: one server. Its tools are its actions
/// (`<server>/<tool>`, run by `invoke` without a `path`); its resources are
/// read by its own action; `read` gives its instructions and the tools'
/// input schemas.
struct McpServer;

/// The arguments of `mcp.spawn`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    /// Start a new process even though the server speaks MCP 2026-07-28 and
    /// so keeps no state per connection (stdio servers only).
    #[serde(default)]
    force: bool,
}

/// The arguments of `mcp.read_resource`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadResourceArgs {
    /// The resource's URI: listed, from a template, or from a tool result.
    uri: String,
}

#[async_trait]
impl Node for McpServer {
    fn about(&self) -> &'static str {
        "one MCP server: its tools are its actions"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        let actions = self.actions().iter().map(ActionSpec::line);
        crate::mcp::list_server(&hub, &servers, at.param("server"), actions).await
    }

    /// Its listing is its own: the actions are among its tools'.
    async fn listing(&self, at: &At<'_>) -> Out {
        self.list(at).await
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        let out = crate::mcp::read_server(&hub, &servers, at.param("server")).await?;
        slice_lines(out.content.text, offset, limit)
            .map(ToolOutput::ok)
            .map_err(ToolError::model)
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        let limit = limit.unwrap_or(crate::mcp::SEARCH_LIMIT);
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        crate::mcp::search(&hub, &servers, at.opt("server"), query, limit).await
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[
            ActionSpec {
                name: "mcp.read_resource",
                about: "read one of its resources (`resources/read`); media land in `@scratch`",
                args: rumu_core::tool_schema::<ReadResourceArgs>,
                approval: Approval::Never,
            },
            ActionSpec {
                name: "mcp.spawn",
                about: "open a connection of this session's own to it (a new process for a \
                        stdio server), for a server that keeps state; returns the instance's name",
                args: rumu_core::tool_schema::<SpawnArgs>,
                approval: Approval::Never,
            },
        ]
    }

    /// Close an instance.
    async fn delete(&self, at: &At<'_>) -> Out {
        crate::mcp::close(&at.p.mcp(), &at.cx.session, at.param("server"))
    }

    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        let (hub, servers) = (at.p.mcp(), at.p.servers(at.cx)?);
        if spec.name == "mcp.spawn" {
            let SpawnArgs { force } = spec.parse(args)?;
            let name = at.param("server");
            return crate::mcp::spawn(&hub, &servers, &at.cx.session, name, force).await;
        }
        let ReadResourceArgs { uri } = spec.parse(args)?;
        let blobs = || at.p.blobs();
        crate::mcp::read_resource(&hub, &servers, at.state, blobs, at.param("server"), &uri).await
    }
}

/// `/skills[/<name>[/<path>]]`: the session's skills, or a skill, or a file
/// or directory in one.
struct SkillFiles;

#[async_trait]
impl Node for SkillFiles {
    fn about(&self) -> &'static str {
        "the skills enabled for this session: instructions to load when a task needs one"
    }

    fn doc(&self, at: &At<'_>) -> &'static str {
        if at.opt("name").is_some() {
            return "";
        }
        "The skills enabled for this session, each with what it is for. `read \
         @rumu:/skills/<name>` loads one: its SKILL.md, then a list of its other files, which \
         are `@rumu:/skills/<name>/<path>`. Read-only."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (skills, rules) = (at.p.skills(), at.p.skill_rules(at.cx)?);
        match at.opt("name") {
            None => Ok(crate::skills::list_skills(&skills, &rules)),
            Some(name) => crate::skills::list(&skills, &rules, at.abs, name, at.param("rest")),
        }
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        let out = match at.opt("name") {
            None => self.listing(at).await?,
            Some(name) => {
                let (skills, rules) = (at.p.skills(), at.p.skill_rules(at.cx)?);
                crate::skills::read(&skills, &rules, at.abs, name, at.param("rest"))?
            }
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
        // `branches` is a static segment beside `{what}`: it wins.
        assert_eq!(params("/sessions/s1/branches"), "sid=s1");
        assert_eq!(params("/sessions/s1/branches/b1"), "sid=s1 bid=b1");
        assert_eq!(params("/cards/self/notes/x.md"), "path=self/notes/x.md");
        assert!(ROUTER.at("/sessions/s1/a/b").is_err());
        let roots = "cards decks sessions skills tasks mcp workspaces";
        assert_eq!(ROOTS.join(" "), roots);
    }

    #[test]
    fn actions_have_unique_full_names() {
        // Building the table asserts every name is `<type>.<action>` and
        // declared once.
        assert!(!ACTIONS.is_empty());
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
