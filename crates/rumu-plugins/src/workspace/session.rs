//! The `@rumu:/sessions` part of the `@rumu` workspace: read-only views of
//! past conversations, rendered from `Cx::history`; the actions that start
//! and wait on sessions are in [`super::start`].
//!
//! - `list /sessions` — the latest visible sessions, most recently updated
//!   first (`read` with `offset` / `limit` pages them all);
//! - `list|read /sessions/<s>` — the session overview (its branches, each
//!   by its `@rumu:/sessions/<s>/branches/<b>` address);
//! - `list /sessions/<s>/branches` — the overview's branch lines;
//! - `read /sessions/<s>/branches/<b>` — the branch's own nodes, rendered as
//!   a range: its first node and as many of its latest as fit (`offset` /
//!   `limit` page the whole); `branch.send` on it messages the agent there;
//! - `read /sessions/<s>/<node>` — one node, fully rendered;
//! - `read /sessions/<s>/<a>..<b>` — a path fragment from `a` down to `b`
//!   (`a` must be `b` or one of its ancestors), root→head order.
//!
//! Nodes render as `<node id="…" kind="…" …attrs>…</node>` elements, so
//! assistant markdown headings cannot be confused with record boundaries.
//! Node ids are abbreviated per session (at least [`MIN_ABBR`] chars) and
//! addressed by an exact id, or any unambiguous prefix of [`MIN_PREFIX`]+
//! chars. Range reads clip long tool-call args, fold nested session reads to
//! an `<elided>` pointer, and keep the head and tail of a long body around a
//! `<truncated>` pointer; single-node reads are never clipped, folded or
//! truncated.

use std::cmp::Reverse;
use std::collections::HashSet;

use async_trait::async_trait;
use rumu_core::{
    Branch, BranchKind, Cx, HistoryHit, Node, NodeBody, PathState, QUOTE_CHARS, RUMU_WORKSPACE,
    SessionId, SessionInfo, ToolCall, ToolError, ToolOutput, ToolResult, quote,
};
use serde_json::Value;

use super::WorkspacePlugin;
use super::time::iso_local;
use super::tools::slice_lines;
use super::vfs::{self, ActionSpec, Approval, At, NoArgs, Out, bad_scope};

/// The `@rumu` result prefix.
const WS: &str = RUMU_WORKSPACE;

/// A long body in a *range* read keeps this many chars from each end, with a
/// `<truncated>` pointer between. Single-node reads are uncapped (only the
/// global output truncation applies).
const RANGE_HEAD: usize = 1000;
const RANGE_TAIL: usize = 1000;

/// Range reads clip tool-call args beyond this many chars.
const RANGE_CALL_ARGS: usize = 300;

/// Characters an unpaged branch read keeps (its first node and its latest
/// ones), under the tool output cap so the cap leaves it alone.
const BRANCH_BUDGET: usize = 16_000;

/// Latest nodes of a sub-agent a task summary shows.
const TAIL_NODES: usize = 3;

/// Shortest per-session node-id abbreviation.
const MIN_ABBR: usize = 8;

/// A node address prefix shorter than this does not resolve.
const MIN_PREFIX: usize = 4;

/// The deck a session belongs to: its project deck when it has one, else its
/// assistant deck.
fn owner(info: &SessionInfo) -> &str {
    info.project.as_deref().unwrap_or(&info.assistant)
}

/// Whether this path may see `info`'s session: its owning deck is mounted on
/// the branch. A session's conversation is part of its project's memory, or
/// of its assistant's when it has no project.
pub(super) fn can_see(state: &PathState, info: &SessionInfo) -> bool {
    state.mounts.iter().any(|m| m.deck == owner(info))
}

/// Sessions an unpaged `list /sessions` shows; `read` with `offset` /
/// `limit` pages the rest.
const LIST_SESSIONS: usize = 25;

/// The directory of per-assistant session listings.
const BY_ASSISTANT: &str = "by-assistant";

/// `/sessions` (and, through [`AssistantSessions`],
/// `/sessions/by-assistant/<deck id>`): one line per visible session (or one
/// assistant's), most recently updated first, the latest [`LIST_SESSIONS`]
/// unless paged. The session this call runs in is marked `(current)`.
pub(super) struct Sessions;

#[async_trait]
impl vfs::Node for Sessions {
    fn about(&self) -> &'static str {
        "past conversations you can see: yours and the mounted decks'"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "Past conversations you can see (those of the mounted decks: this project's, or your \
         own outside any project), most recently updated first; `read` with `offset` / `limit` \
         pages them all. `@rumu:/sessions/<session>` is \
         one's overview with its branches as node ranges; `.../<node>` reads one node in full, \
         `.../<from>..<to>` a stretch of a branch (long bodies clipped). A node is named by \
         its id or an unambiguous prefix of 4+ characters. `search` with this scope (or a \
         session's) finds nodes. Read-only; `session.create` starts a new one."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let mut lines = session_lines(at)?;
        if lines.is_empty() {
            return Ok("no sessions yet".into());
        }
        let older = lines.len().saturating_sub(LIST_SESSIONS);
        if older > 0 {
            lines.truncate(LIST_SESSIONS);
            lines.push(format!(
                "… {older} older: `read {WS}:{}` with `offset: {}` pages them",
                at.abs,
                LIST_SESSIONS + 1
            ));
        }
        Ok(lines.join("\n").into())
    }

    /// Unpaged, the listing; with `offset` / `limit`, the lines of every
    /// session.
    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        if offset.is_none() && limit.is_none() {
            return self.listing(at).await;
        }
        page_sessions(at, offset, limit)
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        let limit = limit.unwrap_or(10);
        at.p.session_search(query, at.opt("id"), "", limit, at.cx)
            .await
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[ActionSpec {
            name: "session.create",
            about: "start a session of an assistant (yourself by default, unasked) on `prompt`, \
                    with its own mounts and this project; it runs on by itself, and \
                    `session.wait` gets its reply",
            args: rumu_core::tool_schema::<super::start::CreateArgs>,
            approval: Approval::Asks(rumu_cards::deck::Action::Ask),
        }]
    }

    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        at.p.create_session(at, spec.parse(args)?).await
    }
}

/// One line per visible session (or `/sessions/by-assistant/<id>`'s
/// assistant's), most recently updated first.
fn session_lines(at: &At<'_>) -> ToolResult<Vec<String>> {
    let mut sessions = visible_sessions(at.cx, at.opt("id"))?;
    sessions.sort_by_key(|s| Reverse(s.updated_at));
    let mut lines = Vec::with_capacity(sessions.len());
    for s in &sessions {
        let current = if s.id == at.cx.session {
            "  (current)"
        } else {
            ""
        };
        lines.push(format!(
            "{WS}:/sessions/{}  {}  \"{}\"{current}{}",
            s.id,
            iso_local(s.updated_at),
            s.title,
            at.p.session_owner(s, at.cx)?
        ));
    }
    Ok(lines)
}

/// `read` of a session listing with `offset` / `limit`: those lines of
/// [`session_lines`].
fn page_sessions(at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
    slice_lines(session_lines(at)?.join("\n"), offset, limit)
        .map(ToolOutput::ok)
        .map_err(ToolError::model)
}

/// `/sessions/by-assistant/<deck id>`: one assistant's sessions, listed as
/// [`Sessions`] lists them (its actions stay on `/sessions`).
pub(super) struct AssistantSessions;

#[async_trait]
impl vfs::Node for AssistantSessions {
    fn about(&self) -> &'static str {
        Sessions.about()
    }

    async fn list(&self, at: &At<'_>) -> Out {
        Sessions.list(at).await
    }

    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        if offset.is_none() && limit.is_none() {
            return self.listing(at).await;
        }
        page_sessions(at, offset, limit)
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        Sessions.search(at, query, limit).await
    }
}

/// `/sessions/by-assistant`: one line per assistant with visible sessions,
/// this path's own first.
pub(super) struct ByAssistant;

#[async_trait]
impl vfs::Node for ByAssistant {
    fn about(&self) -> &'static str {
        "the same sessions, by assistant"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let own = at.state.assistant.clone();
        let mut ids: Vec<String> = visible_sessions(at.cx, None)?
            .into_iter()
            .map(|s| s.assistant)
            .collect();
        ids.sort_by_key(|id| (*id != own, id.clone()));
        ids.dedup();
        if ids.is_empty() {
            return Ok("no sessions yet".into());
        }
        let lines: Vec<String> = ids
            .iter()
            .map(|id| {
                let name = at.p.decks().get(id).map(|d| d.meta.name);
                let you = if *id == own { " (you)" } else { "" };
                format!(
                    "{WS}:/sessions/{BY_ASSISTANT}/{id}  {}{you}",
                    name.unwrap_or_default()
                )
            })
            .collect();
        Ok(lines.join("\n").into())
    }
}

/// `/sessions/<session>[/<node or range>]`: the session overview, one node,
/// or a range `<a>..<b>` of nodes.
pub(super) struct Session;

#[async_trait]
impl vfs::Node for Session {
    fn about(&self) -> &'static str {
        "one session's overview, node or range"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (sid, what) = (at.param("sid"), at.param("what"));
        let (info, nodes, branches) = match at.cx.history().session_tree(&SessionId::from(sid)) {
            Ok(t) => t,
            Err(e) => return Err(ToolError::model(format!("no session `{sid}`: {e}"))),
        };
        if !can_see(at.state, &info) {
            return Err(ToolError::model(format!("no session `{sid}`")));
        }
        let text = match what {
            // The overview.
            "" => render_overview(&info, &branches),
            // A range `<a>..<b>` or a single node.
            node if node.contains("..") => {
                let (a, b) = node.split_once("..").expect("checked for `..`");
                let from = find_node(&nodes, a, sid)?;
                let to = find_node(&nodes, b, sid)?;
                // Walk from `b` up to `a`; it must be `b` itself or an
                // ancestor of it.
                let mut range = vec![to];
                let mut cur = to;
                while cur.id != from.id {
                    let Some(parent) = cur.parent.as_ref().and_then(|p| node_by_id(&nodes, &p.0))
                    else {
                        return Err(ToolError::model(format!(
                            "cannot read `{WS}:/sessions/{sid}/{node}`: \
                             `{a}` is not `{b}` or an ancestor of it"
                        )));
                    };
                    range.push(parent);
                    cur = parent;
                }
                range.reverse();
                SessionView::new(sid, &nodes).render(&range, true)
            }
            node => {
                let n = find_node(&nodes, node, sid)?;
                SessionView::new(sid, &nodes).render(&[n], false)
            }
        };
        Ok(text.into())
    }

    async fn search(&self, at: &At<'_>, query: &str, limit: Option<usize>) -> Out {
        let (sid, limit) = (at.param("sid"), limit.unwrap_or(10));
        at.p.session_search(query, None, sid, limit, at.cx).await
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[ActionSpec {
            name: "session.wait",
            about: "wait until it is idle and return its final reply; with `background: true`, \
                    a note brings the reply when it is done",
            args: rumu_core::tool_schema::<NoArgs>,
            approval: Approval::Never,
        }]
    }

    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        let NoArgs {} = spec.parse(args)?;
        at.p.wait_session(at).await
    }
}

/// `/sessions/<session>/<node or range>`: read as [`Session`] reads it (its
/// actions stay on the session).
pub(super) struct SessionPart;

#[async_trait]
impl vfs::Node for SessionPart {
    fn about(&self) -> &'static str {
        Session.about()
    }

    async fn list(&self, at: &At<'_>) -> Out {
        Session.list(at).await
    }

    async fn search(&self, _at: &At<'_>, _query: &str, _limit: Option<usize>) -> Out {
        Err(bad_scope())
    }
}

/// `/sessions/<session>/branches`: the session's branches, one line each
/// (the overview's `branches:` part).
pub(super) struct Branches;

#[async_trait]
impl vfs::Node for Branches {
    fn about(&self) -> &'static str {
        "the session's branches (main, sub-agents, side questions)"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let sid = at.param("sid");
        let (info, _, branches) = match at.cx.history().session_tree(&SessionId::from(sid)) {
            Ok(t) => t,
            Err(_) => return Err(ToolError::model(format!("no session `{sid}`"))),
        };
        if !can_see(at.state, &info) {
            return Err(ToolError::model(format!("no session `{sid}`")));
        }
        Ok(branch_lines(&info, &branches).into())
    }

    async fn search(&self, _at: &At<'_>, _query: &str, _limit: Option<usize>) -> Out {
        Err(bad_scope())
    }
}

/// `/sessions/<session>/branches/<branch>`: one agent's branch — its own
/// nodes to read, and `branch.send` to message it.
pub(super) struct BranchNode;

#[async_trait]
impl vfs::Node for BranchNode {
    fn about(&self) -> &'static str {
        "one agent's branch: its own transcript (its task and latest nodes; `offset`/`limit` \
         page the whole); `branch.send` messages it"
    }

    /// The transcript in about [`BRANCH_BUDGET`] chars: its task and its
    /// latest nodes.
    async fn list(&self, at: &At<'_>) -> Out {
        Ok(branch_transcript(at, Some(BRANCH_BUDGET))?.into())
    }

    /// Unpaged, the listing; with `offset` / `limit`, lines of the whole
    /// transcript.
    async fn read(&self, at: &At<'_>, offset: Option<u64>, limit: Option<u64>) -> Out {
        if offset.is_none() && limit.is_none() {
            return self.listing(at).await;
        }
        slice_lines(branch_transcript(at, None)?, offset, limit)
            .map(ToolOutput::ok)
            .map_err(ToolError::model)
    }

    async fn search(&self, _at: &At<'_>, _query: &str, _limit: Option<usize>) -> Out {
        Err(bad_scope())
    }

    fn actions(&self) -> &'static [ActionSpec] {
        &[ActionSpec {
            name: "branch.send",
            about: "message the agent on it: into the turn it is running, or starting one; \
                    another assistant's session asks the user first",
            args: rumu_core::tool_schema::<super::start::SendArgs>,
            approval: Approval::Asks(rumu_cards::deck::Action::Ask),
        }]
    }

    async fn invoke(&self, at: &At<'_>, spec: &ActionSpec, args: Value) -> Out {
        at.p.send_branch(at, spec.parse(args)?).await
    }
}

/// The branch `at` names, rendered as a range: all of it, or within
/// `budget` chars ([`SessionView::render_fitting`]).
fn branch_transcript(at: &At<'_>, budget: Option<usize>) -> ToolResult<String> {
    let (sid, bid) = (at.param("sid"), at.param("bid"));
    let (info, nodes, branches) = match at.cx.history().session_tree(&SessionId::from(sid)) {
        Ok(t) => t,
        Err(_) => return Err(ToolError::model(format!("no session `{sid}`"))),
    };
    if !can_see(at.state, &info) {
        return Err(ToolError::model(format!("no session `{sid}`")));
    }
    let b = find_branch(&branches, bid, sid)?;
    let own = own_nodes(&nodes, b);
    let view = SessionView::new(sid, &nodes);
    Ok(match budget {
        Some(budget) => view.render_fitting(&own, budget),
        None => view.render(&own, true),
    })
}

/// The latest few nodes of a sub-agent's branch in this session, for a task
/// summary: its words, calls and results (and system notes), as a range
/// read renders them, oldest first; `None` without such nodes.
pub(super) fn branch_tail(cx: &Cx, branch: &rumu_core::BranchId) -> ToolResult<Option<String>> {
    let sid = cx.session.to_string();
    let (_, nodes, branches) = cx
        .history()
        .session_tree(&cx.session)
        .map_err(|e| ToolError::model(e.to_string()))?;
    let Some(b) = branches.iter().find(|b| &b.id == branch) else {
        return Ok(None);
    };
    let own = own_nodes(&nodes, b);
    let shown: Vec<&Node> = own
        .into_iter()
        .filter(|n| match &n.body {
            NodeBody::Note { source, content } => {
                source == rumu_core::SYSTEM_SOURCE && !content.text.is_empty()
            }
            _ => true,
        })
        .collect();
    if shown.is_empty() {
        return Ok(None);
    }
    let from = shown.len().saturating_sub(TAIL_NODES);
    Ok(Some(
        SessionView::new(&sid, &nodes).render(&shown[from..], true),
    ))
}

/// The sessions this path may see, optionally only `only`'s (an assistant
/// deck id, which must have some).
fn visible_sessions(cx: &Cx, only: Option<&str>) -> ToolResult<Vec<SessionInfo>> {
    let state = cx.state()?;
    let mut sessions = cx.history().sessions()?;
    sessions.retain(|s| can_see(&state, s));
    if let Some(id) = only {
        sessions.retain(|s| s.assistant == id);
        if sessions.is_empty() {
            return Err(ToolError::model(format!(
                "assistant `{id}` has no visible sessions; \
                 `@rumu:/sessions/by-assistant` lists those that do"
            )));
        }
    }
    Ok(sessions)
}

impl WorkspacePlugin {
    /// The ` by <assistant>` suffix of a session line, for another assistant
    /// deck's session (as catalog lines name another author).
    fn session_owner(&self, info: &SessionInfo, cx: &Cx) -> ToolResult<String> {
        if info.assistant == cx.state()?.assistant {
            return Ok(String::new());
        }
        let name = self
            .decks()
            .get(&info.assistant)
            .map_or_else(|| info.assistant.clone(), |d| d.meta.name.clone());
        Ok(format!("  by {name}"))
    }

    /// The session half of `search`: the history index's BM25-ranked nodes
    /// (see `rumu_search`) over user text, assistant text + tool-call args,
    /// tool-result text and note text. Any query word matches (OR); equal
    /// scores are newest first, and tool results are downweighted.
    /// Read-only — no `card.use` markers.
    pub(super) async fn session_search(
        &self,
        query: &str,
        assistant: Option<&str>,
        only: &str,
        limit: usize,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if query.trim().is_empty() {
            return Err(ToolError::model("search: empty query"));
        }
        let mut sessions = visible_sessions(cx, assistant)?;
        if !only.is_empty() {
            sessions.retain(|s| s.id.0 == only);
        }
        let ids: Vec<SessionId> = sessions.iter().map(|s| s.id.clone()).collect();
        let found = cx.history().search(query, &ids, limit)?;
        if found.hits.is_empty() {
            return Ok(format!("no conversation nodes match `{query}`").into());
        }
        // Group the hits by session, sessions ordered by their best hit and
        // nodes by rank within one; a snippet repeated in the same session
        // points back at its first node.
        let mut groups: Vec<(&SessionInfo, Vec<&HistoryHit>)> = Vec::new();
        for hit in &found.hits {
            let Some(info) = sessions.iter().find(|s| s.id == hit.session) else {
                continue;
            };
            match groups.iter_mut().find(|(s, _)| s.id == info.id) {
                Some((_, g)) => g.push(hit),
                None => groups.push((info, vec![hit])),
            }
        }
        let mut out = Vec::new();
        for (info, group) in groups {
            out.push(format!(
                "session {WS}:/sessions/{}  {}{}",
                info.id,
                quote(&info.title, TITLE_CHARS),
                self.session_owner(info, cx)?
            ));
            let ids = cx.history().indexed_nodes(&info.id)?;
            let abbr = abbreviation_len(ids.iter().map(String::as_str));
            let mut seen: Vec<(String, String)> = Vec::new();
            for hit in group {
                let node = abbreviate(&hit.node, abbr);
                let mut line = format!("  {node}  {:<11}  {}", hit.kind, iso_local(hit.at));
                if let Some(s) = rumu_cards::body_snippet(&hit.text, &found.terms) {
                    match seen.iter().find(|(prev, _)| *prev == s) {
                        Some((_, first)) => line.push_str(&format!("  (same as {first})")),
                        None => {
                            line.push_str("  ");
                            line.push_str(&quote(&s, usize::MAX));
                            seen.push((s, node));
                        }
                    }
                }
                out.push(line);
            }
        }
        let mut text = out.join("\n");
        if found.total > found.hits.len() {
            text.push_str(&format!(
                "\n(+{} more hits)",
                found.total - found.hits.len()
            ));
        }
        Ok(text.into())
    }
}

/// Title chars shown in a session-search group header.
const TITLE_CHARS: usize = 48;

fn no_node_err(sid: &str, node: &str) -> ToolError {
    ToolError::model(format!("no node `{node}` on session `{sid}`"))
}

/// Resolve a node address: the full id, else a unique prefix of at least
/// [`MIN_PREFIX`] chars. Ambiguous prefixes and misses are errors.
fn find_node<'a>(nodes: &'a [Node], id: &str, sid: &str) -> ToolResult<&'a Node> {
    if let Some(n) = node_by_id(nodes, id) {
        return Ok(n);
    }
    let matches: Vec<&Node> = if id.chars().count() >= MIN_PREFIX {
        nodes.iter().filter(|n| n.id.0.starts_with(id)).collect()
    } else {
        Vec::new()
    };
    match matches.len() {
        0 => Err(no_node_err(sid, id)),
        1 => Ok(matches[0]),
        _ => Err(ToolError::model(format!(
            "node prefix `{id}` is ambiguous on session `{sid}`: {}",
            matches
                .iter()
                .map(|n| n.id.0.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn node_by_id<'a>(nodes: &'a [Node], id: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.id.0 == id)
}

/// The per-session rendering context: the id, every node (for ancestor
/// lookups) and the session's id abbreviation length.
struct SessionView<'a> {
    sid: &'a str,
    nodes: &'a [Node],
    abbr: usize,
}

impl<'a> SessionView<'a> {
    fn new(sid: &'a str, nodes: &'a [Node]) -> Self {
        Self {
            sid,
            nodes,
            abbr: abbreviation_len(nodes.iter().map(|n| n.id.0.as_str())),
        }
    }

    /// The shortest unique id prefix (or the whole id, if shorter).
    fn short(&self, id: &str) -> String {
        abbreviate(id, self.abbr)
    }

    /// Render `selected` nodes in order, each element followed by a newline.
    fn render(&self, selected: &[&Node], range: bool) -> String {
        let mut out = String::new();
        for n in selected {
            out.push_str(&self.render_node(n, range));
            out.push('\n');
        }
        out
    }

    /// [`Self::render`] as a range, in at most about `budget` chars: when
    /// the whole does not fit, the first node (a sub-agent's task) and as
    /// many of the latest as fit, around an `<omitted>` element that
    /// addresses the nodes left out as a range read. A transcript is read
    /// for where it stands now, which a cut in the middle of the text loses.
    fn render_fitting(&self, selected: &[&Node], budget: usize) -> String {
        let rendered: Vec<String> = selected
            .iter()
            .map(|n| format!("{}\n", self.render_node(n, true)))
            .collect();
        let size = |s: &String| s.chars().count();
        if rendered.iter().map(size).sum::<usize>() <= budget || rendered.len() < 3 {
            return rendered.concat();
        }
        // Room for the omission element itself.
        let mut used = size(&rendered[0]) + 200;
        let mut from = rendered.len();
        while from > 2 && used + size(&rendered[from - 1]) <= budget {
            from -= 1;
            used += size(&rendered[from]);
        }
        // Always keep the head itself, even when it alone is over budget.
        if from == rendered.len() {
            from -= 1;
        }
        let (a, b) = (&selected[1].id.0, &selected[from - 1].id.0);
        let omitted = format!(
            "<omitted nodes=\"{}\" read=\"{WS}:/sessions/{}/{}..{}\"/>\n",
            from - 1,
            self.sid,
            self.short(a),
            self.short(b)
        );
        let mut out = rendered[0].clone();
        out.push_str(&omitted);
        out.push_str(&rendered[from..].concat());
        out
    }

    /// One node as a `<node>` element. Range reads cap bodies, clip call
    /// args, and fold nested session reads; single-node reads do none.
    fn render_node(&self, n: &Node, range: bool) -> String {
        let short = self.short(&n.id.0);
        let (kind, attrs, body) = match &n.body {
            NodeBody::User { content } => (
                "user",
                String::new(),
                self.text(&content.rendered(), range, &short),
            ),
            NodeBody::Assistant {
                content,
                tool_calls,
                ..
            } => {
                // Only the prose is truncated: cutting across `<call>` lines
                // could split a tag. Each call clips its own args instead.
                let mut body = self.text(&content.rendered(), range, &short);
                for call in tool_calls {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&render_call(call, range));
                }
                ("assistant", String::new(), body)
            }
            NodeBody::ToolResult {
                call_id,
                content,
                is_error,
                invoke,
            } => {
                let call = self.matching_call(n, call_id);
                let mut attrs = format!(" call=\"{}\"", escape_attr(call_id));
                // An invoke's name/args live on the node (there is no model
                // call to look up); `parent` names the enclosing call.
                let folds = if let Some(inv) = invoke {
                    attrs.push_str(&format!(
                        " tool=\"{}\" parent=\"{}\"",
                        escape_attr(&inv.name),
                        escape_attr(&inv.parent)
                    ));
                    folds_session_read_args(&inv.name, &inv.args)
                } else {
                    if let Some(c) = call {
                        attrs.push_str(&format!(" tool=\"{}\"", escape_attr(&c.name)));
                    }
                    call.is_some_and(folds_session_read)
                };
                if *is_error {
                    attrs.push_str(" error=\"true\"");
                }
                // The elided size counts what the full read would show, so
                // the image placeholders count too.
                let body = if range && folds {
                    format!(
                        "<elided chars=\"{}\" read=\"{WS}:/sessions/{}/{short}\"/>",
                        content.rendered().chars().count(),
                        self.sid
                    )
                } else {
                    self.text(&content.rendered(), range, &short)
                };
                ("tool_result", attrs, body)
            }
            NodeBody::Note { source, content } => {
                // A marker: data only, one line.
                if content.text.is_empty()
                    && let Some(tagged) = &content.data
                {
                    return format!(
                        "<node id=\"{short}\" kind=\"marker\" tag=\"{}\">{}</node>",
                        escape_attr(&tagged.tag),
                        escape_body(&one_line_json(&tagged.data))
                    );
                }
                let mut attrs = format!(" source=\"{}\"", escape_attr(source));
                if let Some(tagged) = &content.data {
                    attrs.push_str(&format!(" tag=\"{}\"", escape_attr(&tagged.tag)));
                }
                ("note", attrs, self.text(&content.rendered(), range, &short))
            }
        };
        let open = format!("<node id=\"{short}\" kind=\"{kind}\"{attrs}>");
        if body.is_empty() {
            format!("{open}</node>")
        } else {
            format!("{open}\n{body}\n</node>")
        }
    }

    /// The assistant call this tool result answers: walk ancestors to a
    /// node with the matching `call_id` (parallel results sit under each
    /// other: T2→T1→A1).
    fn matching_call(&self, n: &Node, call_id: &str) -> Option<&'a ToolCall> {
        let mut cur = n.parent.clone()?;
        loop {
            let node = node_by_id(self.nodes, &cur.0)?;
            if let NodeBody::Assistant { tool_calls, .. } = &node.body
                && let Some(call) = tool_calls.iter().find(|c| c.call_id == call_id)
            {
                return Some(call);
            }
            cur = node.parent.clone()?;
        }
    }

    /// Escaped body text; in range reads a long one keeps its head and tail
    /// around a `<truncated>` pointer to the single-node read.
    fn text(&self, text: &str, range: bool, short: &str) -> String {
        let total = text.chars().count();
        if !range || total <= RANGE_HEAD + RANGE_TAIL {
            return escape_body(text);
        }
        let head: String = text.chars().take(RANGE_HEAD).collect();
        let tail: String = text.chars().skip(total - RANGE_TAIL).collect();
        format!(
            "{}\n<truncated chars=\"{}\" read=\"{WS}:/sessions/{}/{short}\"/>\n{}",
            escape_body(&head),
            total - RANGE_HEAD - RANGE_TAIL,
            self.sid,
            escape_body(&tail)
        )
    }
}

/// One `<call>` line, its args clipped in range reads.
fn render_call(call: &ToolCall, range: bool) -> String {
    let args = one_line_json(&call.args);
    let len = args.chars().count();
    let mut attrs = format!(
        " id=\"{}\" name=\"{}\"",
        escape_attr(&call.call_id),
        escape_attr(&call.name)
    );
    let args = if range && len > RANGE_CALL_ARGS {
        attrs.push_str(&format!(" truncated=\"{}\"", len - RANGE_CALL_ARGS));
        args.chars().take(RANGE_CALL_ARGS).collect()
    } else {
        args
    };
    format!("<call{attrs}>{}</call>", escape_body(&args))
}

/// Whether a tool result is a session read/list/search that a range read can
/// fold to an `<elided>` pointer. Only absolute `@rumu:/sessions…` arguments
/// are recognized; relative or default-workspace forms (e.g. `sessions/…`)
/// are not detected — accepted.
fn folds_session_read(call: &ToolCall) -> bool {
    folds_session_read_args(&call.name, &call.args)
}

/// [`folds_session_read`] over a bare name/args pair (an invoke carries those
/// without a model [`ToolCall`]).
fn folds_session_read_args(name: &str, args: &Value) -> bool {
    let arg = match name {
        "read" | "list" => args.get("path"),
        "search" => args.get("scope"),
        _ => return false,
    };
    arg.and_then(Value::as_str)
        .is_some_and(|s| s.starts_with(&format!("{WS}:/sessions")))
}

/// Escape `<` where body text could forge our markup. Every other `<` (and
/// every other character) is left alone.
fn escape_body(s: &str) -> String {
    const NEEDLES: [&str; 6] = [
        "<node",
        "</node",
        "<call",
        "</call",
        "<truncated",
        "<elided",
    ];
    let mut out = String::with_capacity(s.len());
    for (i, ch) in s.char_indices() {
        if ch == '<' && NEEDLES.iter().any(|n| s[i..].starts_with(n)) {
            out.push_str("&lt;");
        } else {
            out.push(ch);
        }
    }
    out
}

/// Escape a value we emit as an attribute.
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '<' => out.push_str("&lt;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Per session, the smallest id prefix length (at least [`MIN_ABBR`]) that
/// keeps every node's abbreviation distinct.
fn abbreviation_len<'a>(ids: impl Iterator<Item = &'a str> + Clone) -> usize {
    let mut len = MIN_ABBR;
    loop {
        let mut seen = HashSet::new();
        if ids.clone().all(|id| seen.insert(abbreviate(id, len))) {
            return len;
        }
        len += 1;
    }
}

fn abbreviate(id: &str, len: usize) -> String {
    id.chars().take(len).collect()
}

/// The session overview: `key: value` header lines plus one line per
/// branch, sorted by `created_at`, each with its address.
fn render_overview(info: &SessionInfo, branches: &[Branch]) -> String {
    let mut out = format!(
        "session: {}\ntitle: {}\nupdated: {}\nbranches:\n",
        info.id,
        quote(&info.title, QUOTE_CHARS),
        iso_local(info.updated_at)
    );
    out.push_str(&branch_lines(info, branches));
    out
}

/// One line per branch of the session, sorted by `created_at`: kind,
/// status, address, and a sub-agent's task.
fn branch_lines(info: &SessionInfo, branches: &[Branch]) -> String {
    let mut out = Vec::new();
    let mut sorted: Vec<&Branch> = branches.iter().collect();
    sorted.sort_by_key(|b| b.created_at);
    for b in sorted {
        let kind = match &b.kind {
            BranchKind::Main => "main",
            BranchKind::Agent { .. } => "agent",
            BranchKind::Side { .. } => "side",
        };
        let status = match b.status {
            rumu_core::BranchStatus::Active => "active",
            rumu_core::BranchStatus::Done => "done",
        };
        let mut line = format!(
            "  {kind:<8} {status:<10} {}",
            rumu_core::branch_address(&info.id, &b.id)
        );
        if let BranchKind::Agent { task, .. } = &b.kind {
            line.push_str(&format!("  task {}", quote(task, QUOTE_CHARS)));
        }
        out.push(line);
    }
    out.join("\n")
}

/// The branch's own nodes, root→head: from its head up to the node it
/// forked from (all the way to the root for the main line).
fn own_nodes<'a>(nodes: &'a [Node], b: &Branch) -> Vec<&'a Node> {
    let mut own = Vec::new();
    let mut cur = node_by_id(nodes, &b.head.0);
    while let Some(n) = cur.filter(|n| Some(&n.id) != b.forked_from.as_ref()) {
        own.push(n);
        cur = n.parent.as_ref().and_then(|p| node_by_id(nodes, &p.0));
    }
    own.reverse();
    own
}

/// The branch an address names in a session: by id, or a unique prefix of
/// at least [`MIN_PREFIX`] chars.
pub(super) fn find_branch<'a>(
    branches: &'a [Branch],
    id: &str,
    sid: &str,
) -> ToolResult<&'a Branch> {
    let matches: Vec<&Branch> = match branches.iter().find(|b| b.id.0 == id) {
        Some(b) => vec![b],
        None if id.chars().count() >= MIN_PREFIX => {
            branches.iter().filter(|b| b.id.0.starts_with(id)).collect()
        }
        None => Vec::new(),
    };
    match matches[..] {
        [b] => Ok(b),
        [] => Err(ToolError::model(format!(
            "no branch `{id}` on session `{sid}`"
        ))),
        _ => Err(ToolError::model(format!(
            "branch prefix `{id}` is ambiguous on session `{sid}`"
        ))),
    }
}

/// Compact single-line JSON (no spaces after `:` and `,`).
fn one_line_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A long transcript keeps its first node and its latest ones, and
    /// addresses what it leaves out as a range read.
    #[test]
    fn fitting_render_keeps_the_task_and_the_latest_nodes() {
        let sid = SessionId::from("S");
        let nodes: Vec<Node> = (0..10)
            .map(|i| Node {
                id: rumu_core::NodeId::from(format!("N{i}").as_str()),
                parent: (i > 0).then(|| rumu_core::NodeId::from(format!("N{}", i - 1).as_str())),
                session: sid.clone(),
                at: i,
                dur_ms: None,
                body: NodeBody::User {
                    content: rumu_core::Content::text(format!("{i}{}", "x".repeat(900))),
                },
            })
            .collect();
        let all: Vec<&Node> = nodes.iter().collect();
        let view = SessionView::new("S", &nodes);
        assert_eq!(view.render_fitting(&all, 100_000), view.render(&all, true));

        let out = view.render_fitting(&all, 3_500);
        assert!(out.starts_with("<node id=\"N0\""), "{out}");
        assert!(
            out.contains(
                "<omitted nodes=\"7\" read=\"@rumu:/sessions/S/N1..N7\"/>\n<node id=\"N8\""
            ),
            "{out}"
        );
        assert!(out.trim_end().ends_with("</node>") && out.contains("id=\"N9\""));
    }
}
