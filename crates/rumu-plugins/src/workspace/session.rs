//! The `@rumu:/sessions` part of the `@rumu` workspace: read-only views of
//! past conversations, rendered from `Cx::history`; the actions that start
//! and wait on sessions are in [`super::start`].
//!
//! - `list /sessions` — every session of this assistant, most recently updated first;
//! - `list|read /sessions/<s>` — the session overview (its branches, each
//!   by its `@rumu:/sessions/<s>/branches/<b>` address);
//! - `read /sessions/<s>/branches/<b>` — the branch's own nodes, rendered as
//!   a range; `branch.send` on it messages the agent there;
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
use rumu_cards::json_quote;
use rumu_cards::recall::{self, Doc};
use rumu_core::{
    Branch, BranchKind, Cx, Node, NodeBody, PathState, RUMU_WORKSPACE, SessionId, SessionInfo,
    ToolCall, ToolError, ToolOutput, ToolResult,
};
use serde_json::Value;

use super::WorkspacePlugin;
use super::time::iso_local;
use super::vfs::{self, ActionSpec, Approval, At, NoArgs, Out, bad_scope};

/// The `@rumu` result prefix.
const WS: &str = RUMU_WORKSPACE;

/// Tool results are long and log-heavy: halve their score so prose nodes with
/// the same match rank above them.
const TOOL_RESULT_BOOST: f64 = 0.5;

/// A long body in a *range* read keeps this many chars from each end, with a
/// `<truncated>` pointer between. Single-node reads are uncapped (only the
/// global output truncation applies).
const RANGE_HEAD: usize = 1000;
const RANGE_TAIL: usize = 1000;

/// Range reads clip tool-call args beyond this many chars.
const RANGE_CALL_ARGS: usize = 300;

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

/// The directory of per-assistant session listings.
const BY_ASSISTANT: &str = "by-assistant";

/// `/sessions` (and, through [`AssistantSessions`],
/// `/sessions/by-assistant/<deck id>`): one line per visible session (or one
/// assistant's), most recently updated first. The session this call runs in
/// is marked `(current)`.
pub(super) struct Sessions;

#[async_trait]
impl vfs::Node for Sessions {
    fn about(&self) -> &'static str {
        "past conversations you can see: yours and the mounted decks'"
    }

    fn doc(&self, _at: &At<'_>) -> &'static str {
        "Past conversations you can see (those of the mounted decks: this project's, or your \
         own outside any project), most recently updated first. `@rumu:/sessions/<session>` is \
         one's overview with its branches as node ranges; `.../<node>` reads one node in full, \
         `.../<from>..<to>` a stretch of a branch (long bodies clipped). A node is named by \
         its id or an unambiguous prefix of 4+ characters. `search` with this scope (or a \
         session's) finds nodes. Read-only; `session.create` starts a new one."
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let mut sessions = visible_sessions(at.cx, at.opt("id"))?;
        sessions.sort_by_key(|s| Reverse(s.updated_at));
        if sessions.is_empty() {
            return Ok("no sessions yet".into());
        }
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
        Ok(lines.join("\n").into())
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

/// `/sessions/<session>/branches/<branch>`: one agent's branch — its own
/// nodes to read, and `branch.send` to message it.
pub(super) struct BranchNode;

#[async_trait]
impl vfs::Node for BranchNode {
    fn about(&self) -> &'static str {
        "one agent's branch: its own transcript; `branch.send` messages it"
    }

    async fn list(&self, at: &At<'_>) -> Out {
        let (sid, bid) = (at.param("sid"), at.param("bid"));
        let (info, nodes, branches) = match at.cx.history().session_tree(&SessionId::from(sid)) {
            Ok(t) => t,
            Err(_) => return Err(ToolError::model(format!("no session `{sid}`"))),
        };
        if !can_see(at.state, &info) {
            return Err(ToolError::model(format!("no session `{sid}`")));
        }
        let b = find_branch(&branches, bid, sid)?;
        Ok(SessionView::new(sid, &nodes)
            .render(&own_nodes(&nodes, b), true)
            .into())
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

    /// The session half of `search`: BM25-ranked nodes over user text,
    /// assistant text + tool-call args, tool-result text and note text. Any
    /// query term matches (OR); equal scores are newest first by `at`, and
    /// tool results are downweighted. Read-only — no `card_use` markers.
    pub(super) async fn session_search(
        &self,
        query: &str,
        assistant: Option<&str>,
        only: &str,
        limit: usize,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        // Snippet terms need the ranker's tokenization, not whitespace: a
        // spaceless CJK query would otherwise never print a snippet.
        let terms: Vec<String> = recall::tokens(query);
        if query.trim().is_empty() {
            return Err(ToolError::model("search: empty query"));
        }
        let mut sessions = visible_sessions(cx, assistant)?;
        sessions.sort_by_key(|s| Reverse(s.updated_at));

        struct Candidate {
            session: usize,
            node: String,
            at: rumu_core::Timestamp,
            kind: &'static str,
            text: String,
        }
        let mut candidates = Vec::new();
        for (si, info) in sessions.iter().enumerate() {
            if !only.is_empty() && info.id.0 != only {
                continue;
            }
            let Ok((_, nodes, _)) = cx.history().session_tree(&info.id) else {
                continue; // unreadable sessions just have no hits
            };
            let view = SessionView::new(&info.id.0, &nodes);
            for n in &nodes {
                let Some((kind, text)) = searchable_text(&n.body) else {
                    continue;
                };
                candidates.push(Candidate {
                    session: si,
                    node: view.short(&n.id.0),
                    at: n.at,
                    kind,
                    text,
                });
            }
        }
        // Newest first: a stable score sort keeps this order for equal scores.
        candidates.sort_by_key(|c| Reverse(c.at));
        let docs: Vec<Doc> = candidates
            .iter()
            .map(|c| {
                let boost = if c.kind == "tool_result" {
                    TOOL_RESULT_BOOST
                } else {
                    1.0
                };
                Doc::new([(c.text.as_str(), 1.0)], boost)
            })
            .collect();
        // Explicit search: every positive score is a hit, and a term common to
        // every node still finds those nodes.
        let hits = recall::rank_docs(&docs, query, docs.len(), false);

        if hits.is_empty() {
            return Ok(format!("no conversation nodes match `{query}`").into());
        }
        // Group the shown hits by session, sessions ordered by their best hit
        // and nodes by rank within one; a snippet repeated in the same session
        // points back at its first node.
        let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
        for &(i, _) in hits.iter().take(limit) {
            let si = candidates[i].session;
            match groups.iter_mut().find(|(s, _)| *s == si) {
                Some((_, g)) => g.push(i),
                None => groups.push((si, vec![i])),
            }
        }
        let mut out = Vec::new();
        for (si, group) in groups {
            let info = &sessions[si];
            out.push(format!(
                "session {WS}:/sessions/{}  {}{}",
                info.id,
                json_quote(&info.title, TITLE_CHARS),
                self.session_owner(info, cx)?
            ));
            let mut seen: Vec<(String, &str)> = Vec::new();
            for i in group {
                let c = &candidates[i];
                let mut line = format!("  {}  {:<11}  {}", c.node, c.kind, iso_local(c.at));
                if let Some(s) = rumu_cards::body_snippet(&c.text, &terms) {
                    match seen.iter().find(|(prev, _)| *prev == s) {
                        Some((_, first)) => line.push_str(&format!("  (same as {first})")),
                        None => {
                            line.push_str("  ");
                            line.push_str(&json_quote(&s, usize::MAX));
                            seen.push((s, &c.node));
                        }
                    }
                }
                out.push(line);
            }
        }
        let mut text = out.join("\n");
        if hits.len() > limit {
            text.push_str(&format!("\n(+{} more hits)", hits.len() - limit));
        }
        Ok(text.into())
    }
}

/// Title chars shown in a session-search group header.
const TITLE_CHARS: usize = 48;

/// Tools whose call args are not indexed: they only look things up, so their
/// args just echo earlier queries and paths back as noise hits.
const LOOKUP_TOOLS: [&str; 3] = ["read", "list", "search"];

/// The text a session-search term matches against, with the node kind label:
/// user text, assistant text + tool-call args (bar [`LOOKUP_TOOLS`]),
/// tool-result text, note text —
/// images as their `[image: …]` placeholder. Markers carry nothing
/// searchable.
fn searchable_text(body: &NodeBody) -> Option<(&'static str, String)> {
    match body {
        NodeBody::User { content } => Some(("user", content.rendered())),
        NodeBody::Assistant {
            content,
            tool_calls,
            ..
        } => {
            let mut text = content.rendered();
            for call in tool_calls {
                if LOOKUP_TOOLS.contains(&call.name.as_str()) {
                    continue;
                }
                text.push('\n');
                text.push_str(&call.name);
                text.push(' ');
                text.push_str(&serde_json::to_string(&call.args).unwrap_or_default());
            }
            Some(("assistant", text))
        }
        NodeBody::ToolResult { content, .. } => Some(("tool_result", content.rendered())),
        // An empty-text note is a marker: nothing searchable.
        NodeBody::Note { content, .. } if !content.text.is_empty() => {
            Some(("note", content.rendered()))
        }
        NodeBody::Note { .. } => None,
    }
}

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
            abbr: abbreviation_len(nodes),
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
fn abbreviation_len(nodes: &[Node]) -> usize {
    let mut len = MIN_ABBR;
    loop {
        let mut seen = HashSet::new();
        if nodes.iter().all(|n| seen.insert(abbreviate(&n.id.0, len))) {
            return len;
        }
        len += 1;
    }
}

fn abbreviate(id: &str, len: usize) -> String {
    id.chars().take(len).collect()
}

/// The session overview: one header line plus one line per branch,
/// sorted by `created_at`, each with its address.
fn render_overview(info: &SessionInfo, branches: &[Branch]) -> String {
    let mut out = format!(
        "session {} — \"{}\" · updated {}",
        info.id,
        info.title,
        iso_local(info.updated_at)
    );
    out.push_str("\nbranches:");
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
            line.push_str(&format!("  task: {}", clip(task, 80)));
        }
        out.push('\n');
        out.push_str(&line);
    }
    out
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

/// Clip to `max` chars on a char boundary, appending an ellipsis when cut.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Compact single-line JSON (no spaces after `:` and `,`).
fn one_line_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}
