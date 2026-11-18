//! The `@rumu` workspace's dispatch: one table of path patterns
//! ([`matchit`]), each served by a stateless [`Node`]. The file tools
//! resolve an `@rumu` path here and call the node's operation; whatever a
//! node does not serve is refused the same way everywhere.

use std::sync::LazyLock;

use async_trait::async_trait;
use matchit::{Params, Router};
use rumu_core::{Cx, PathState, RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult, Utf8UnixPath};

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
/// unless it says otherwise; writing and searching are refused unless the
/// node serves them.
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

    async fn search(&self, _at: &At<'_>, _query: &str, _limit: Option<usize>) -> Out {
        Err(bad_scope())
    }

    /// The refusal of a write or edit here.
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
}
