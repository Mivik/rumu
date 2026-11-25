//! The session graph the web's branch view draws: the main line's node tree
//! cut into turns (a user message and one version of its reply), where each
//! sub-agent, side question and main-line task hangs from it, and what each
//! sub-agent's own nodes add up to. Derived on request, never stored.

use std::collections::{HashMap, HashSet};

use rumu_core::{Branch, BranchId, BranchKind, Node, NodeBody, NodeId, SessionId, Timestamp};
use serde::Serialize;

use crate::waker::Anchors;
use crate::{Kernel, Result};

/// A side question's text: the opening line of its question, this many
/// chars at most.
const TEXT_CHARS: usize = 120;
/// A turn's text: its message, this many chars at most (the view folds it).
const TURN_CHARS: usize = 1000;

#[derive(Debug, Default, Serialize)]
pub struct Graph {
    /// Oldest first.
    pub turns: Vec<Turn>,
    /// What hangs directly off the main line → the turn it hangs from: a
    /// sub-agent or side question by branch id, a main-line task by task id.
    pub anchors: HashMap<String, NodeId>,
    /// Each sub-agent's and side question's own nodes, summed.
    pub branches: HashMap<BranchId, BranchSummary>,
}

/// A user message of the main line with one version of its reply. The
/// turns under one parent turn are that message's versions: those sharing
/// the message are regenerations, the others edits.
#[derive(Debug, Serialize)]
pub struct Turn {
    /// The reply's first node; the message's own while it has no reply.
    pub id: NodeId,
    /// The turn before it.
    pub parent: Option<NodeId>,
    pub user: NodeId,
    /// When the reply started (the message's time without one).
    pub at: Timestamp,
    pub text: String,
    /// On the main line's path, root → head.
    pub line: bool,
    /// Shares its message with another turn.
    pub regen: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct BranchSummary {
    /// When its last node was written.
    pub last_at: Timestamp,
    /// Model calls.
    pub calls: u32,
    /// Tool calls those made.
    pub tools: u32,
    /// The model of its latest call.
    pub model: Option<String>,
    /// A side question's question: the opening line of its first message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A task of the main line, as its anchor needs it.
pub(crate) struct LineTask {
    pub id: String,
    pub call_id: String,
    pub started_at: Timestamp,
}

impl Kernel {
    /// The branch view's graph of `session`.
    pub fn graph(&self, session: &SessionId) -> Result<Graph> {
        let tasks: Vec<(BranchId, LineTask)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .tasks
                .values()
                .filter(|e| &e.info.session == session)
                .map(|e| {
                    let task = LineTask {
                        id: e.info.id.0.clone(),
                        call_id: e.info.call_id.clone(),
                        started_at: e.info.started_at,
                    };
                    (e.info.branch.clone(), task)
                })
                .collect()
        };
        self.with_tree(session, |info, nodes, branches| {
            let Some(main) = branches.get(&info.main_branch) else {
                return Graph::default();
            };
            let tasks: Vec<&LineTask> = tasks
                .iter()
                .filter(|(b, _)| b == &main.id)
                .map(|(_, t)| t)
                .collect();
            build(main, nodes, branches, &tasks)
        })
    }
}

fn is_user(n: &Node) -> bool {
    matches!(n.body, NodeBody::User { .. })
}

pub(crate) fn build(
    main: &Branch,
    nodes: &HashMap<NodeId, Node>,
    branches: &HashMap<BranchId, Branch>,
    tasks: &[&LineTask],
) -> Graph {
    let off = Kernel::off_line_of(nodes.values(), branches.values());
    let anchors = Anchors::build(&main.id, nodes.values(), branches.values(), &off);
    let mut path = HashSet::new();
    let mut cur = nodes.get(&main.head);
    while let Some(n) = cur {
        path.insert(&n.id);
        cur = n.parent.as_ref().and_then(|p| nodes.get(p));
    }

    // Each main-line node's turn key: a message's own id for the message, the
    // reply's first node for the rest of its reply.
    let mut keys: HashMap<&NodeId, Option<&NodeId>> = HashMap::new();
    let mut key = |start: &NodeId| -> Option<NodeId> {
        let (start, _) = nodes.get_key_value(start)?;
        let mut chain = Vec::new();
        let mut cur = start;
        let k = loop {
            if let Some(k) = keys.get(cur) {
                break *k;
            }
            chain.push(cur);
            let n = &nodes[cur];
            if is_user(n) {
                break Some(cur);
            }
            match n.parent.as_ref().and_then(|p| nodes.get_key_value(p)) {
                None => break None,
                Some((_, p)) if is_user(p) => break Some(cur),
                Some((p, _)) => cur = p,
            }
        };
        for c in chain {
            keys.insert(c, k);
        }
        k.cloned()
    };

    // The reply turns: a node right under a message starts one.
    let mut replies: HashMap<&NodeId, Vec<&Node>> = HashMap::new();
    for n in nodes
        .values()
        .filter(|n| !off.contains(&n.id) && !is_user(n))
    {
        if let Some(u) = n
            .parent
            .as_ref()
            .and_then(|p| nodes.get(p))
            .filter(|p| is_user(p))
        {
            replies.entry(&u.id).or_default().push(n);
        }
    }
    for list in replies.values_mut() {
        list.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    }
    // A message stands as a turn itself while no reply to it does (on the
    // path: none of them is on it).
    let bare = |u: &Node| {
        replies
            .get(&u.id)
            .is_none_or(|r| path.contains(&u.id) && !r.iter().any(|n| path.contains(&n.id)))
    };
    // The turn holding `id`: a message with replies is held by the one on
    // the path, else its latest.
    let turn_of = |k: NodeId| -> NodeId {
        match replies.get(&k) {
            Some(r) if !nodes.get(&k).is_some_and(bare) => r
                .iter()
                .find(|n| path.contains(&n.id))
                .or(r.last())
                .map_or(k, |n| n.id.clone()),
            _ => k,
        }
    };
    let mut at_turn = |id: &NodeId| key(id).map(&turn_of);

    let mut turns = Vec::new();
    for u in nodes
        .values()
        .filter(|n| !off.contains(&n.id) && is_user(n))
    {
        let NodeBody::User { content } = &u.body else {
            continue;
        };
        let text = excerpt(&content.text);
        let parent = u.parent.as_ref().and_then(&mut at_turn);
        let mut versions: Vec<(&NodeId, Timestamp)> = Vec::new();
        if bare(u) {
            versions.push((&u.id, u.at));
        }
        for n in replies.get(&u.id).into_iter().flatten() {
            let start = n.at - n.dur_ms.unwrap_or(0) as Timestamp;
            versions.push((&n.id, start.max(u.at)));
        }
        let regen = versions.len() > 1;
        for (id, at) in versions {
            turns.push(Turn {
                id: id.clone(),
                parent: parent.clone(),
                user: u.id.clone(),
                at,
                text: text.clone(),
                line: path.contains(id),
                regen,
            });
        }
    }
    turns.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));

    let mut out = HashMap::new();
    for b in branches.values() {
        let node = match &b.kind {
            BranchKind::Agent { parent_branch, .. } if parent_branch == &main.id => {
                anchors.agent(&b.id)
            }
            BranchKind::Side { parent_branch, .. } if parent_branch == &main.id => {
                b.forked_from.as_ref()
            }
            _ => None,
        };
        if let Some(t) = node.and_then(&mut at_turn) {
            out.insert(b.id.0.clone(), t);
        }
    }
    for t in tasks {
        // A script's call is `"{model_call}.{seq}"`.
        let node = anchors.call(&t.call_id, t.started_at).or_else(|| {
            let (outer, _) = t.call_id.rsplit_once('.')?;
            anchors.call(outer, t.started_at)
        });
        if let Some(turn) = node.and_then(&mut at_turn) {
            out.insert(t.id.clone(), turn);
        }
    }

    Graph {
        turns,
        anchors: out,
        branches: branches
            .values()
            .filter(|b| !matches!(b.kind, BranchKind::Main))
            .map(|b| (b.id.clone(), summary(b, nodes)))
            .collect(),
    }
}

/// The first non-blank line of `text`, `TEXT_CHARS` at most.
fn opening(text: &str) -> String {
    text.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .chars()
        .take(TEXT_CHARS)
        .collect()
}

/// `text` trimmed, `TURN_CHARS` at most, an ellipsis where it was cut.
fn excerpt(text: &str) -> String {
    let text = text.trim();
    match text.char_indices().nth(TURN_CHARS) {
        Some((i, _)) => format!("{}…", text[..i].trim_end()),
        None => text.to_owned(),
    }
}

/// `b`'s own nodes, its head up to the node it forked from, summed.
fn summary(b: &Branch, nodes: &HashMap<NodeId, Node>) -> BranchSummary {
    let mut s = BranchSummary {
        last_at: b.created_at,
        ..Default::default()
    };
    let mut cur = Some(&b.head);
    while let Some(n) = cur
        .filter(|id| Some(*id) != b.forked_from.as_ref())
        .and_then(|id| nodes.get(id))
    {
        s.last_at = s.last_at.max(n.at);
        if let (NodeBody::User { content }, BranchKind::Side { .. }) = (&n.body, &b.kind) {
            s.text = Some(opening(&content.text));
        }
        if let NodeBody::Assistant {
            tool_calls, model, ..
        } = &n.body
        {
            s.calls += 1;
            s.tools += tool_calls.len() as u32;
            if s.model.is_none() {
                s.model = Some(model.clone());
            }
        }
        cur = n.parent.as_ref();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn node(id: &str, parent: Option<&str>, at: i64, body: Value) -> Node {
        serde_json::from_value(json!({
            "id": id, "parent": parent, "session": "s", "at": at, "body": body,
        }))
        .unwrap()
    }
    fn user(id: &str, parent: Option<&str>, at: i64, text: &str) -> Node {
        node(
            id,
            parent,
            at,
            json!({ "type": "user", "content": { "text": text } }),
        )
    }
    fn reply(id: &str, parent: &str, at: i64, calls: &[&str]) -> Node {
        let calls: Vec<Value> = calls
            .iter()
            .map(|c| json!({ "call_id": c, "name": "agent", "args": {} }))
            .collect();
        node(
            id,
            Some(parent),
            at,
            json!({ "type": "assistant", "content": { "text": "" }, "tool_calls": calls, "model": "m" }),
        )
    }
    fn branch(id: &str, head: &str, forked: Option<&str>, kind: Value) -> Branch {
        serde_json::from_value(json!({
            "id": id, "session": "s", "head": head, "forked_from": forked,
            "kind": kind, "status": "done", "created_at": 0,
        }))
        .unwrap()
    }

    /// root ─ u1 ─┬ a1 (calls x1) ─ u2 ─ a2
    ///            └ b1 (regenerated)
    ///      └ u1' ─ c1 (edited)
    /// with the head on a2, a sub-agent off a1 and one below that.
    #[test]
    fn turns_versions_and_anchors() {
        let nodes: HashMap<NodeId, Node> = [
            node(
                "root",
                None,
                0,
                json!({ "type": "note", "source": "s", "content": { "text": "" } }),
            ),
            user("u1", Some("root"), 10, "\n first line\nsecond"),
            reply("a1", "u1", 20, &["x1"]),
            reply("b1", "u1", 30, &[]),
            user("u1e", Some("root"), 40, "edited"),
            reply("c1", "u1e", 50, &[]),
            user("u2", Some("a1"), 60, "next"),
            reply("a2", "u2", 70, &[]),
            reply("k1", "a1", 25, &["y1"]),
            reply("k2", "k1", 26, &[]),
            user("s1", Some("a1"), 27, "why?"),
        ]
        .into_iter()
        .map(|n| (n.id.clone(), n))
        .collect();
        let branches: HashMap<BranchId, Branch> = [
            branch("main", "a2", None, json!({ "type": "main" })),
            branch(
                "kid",
                "k1",
                Some("a1"),
                json!({ "type": "agent", "task": "t", "parent_branch": "main", "call_id": "x1" }),
            ),
            branch(
                "grandkid",
                "k2",
                Some("k1"),
                json!({ "type": "agent", "task": "t", "parent_branch": "kid", "call_id": "y1" }),
            ),
            branch(
                "side",
                "s1",
                Some("a1"),
                json!({ "type": "side", "parent_branch": "main" }),
            ),
        ]
        .into_iter()
        .map(|b| (b.id.clone(), b))
        .collect();
        let task = LineTask {
            id: "t1".into(),
            call_id: "x1.2".into(),
            started_at: 21,
        };
        let g = build(&branches[&"main".into()], &nodes, &branches, &[&task]);

        let turns: Vec<(&str, Option<&str>, &str, bool, bool)> = g
            .turns
            .iter()
            .map(|t| {
                (
                    t.id.0.as_str(),
                    t.parent.as_ref().map(|p| p.0.as_str()),
                    t.user.0.as_str(),
                    t.line,
                    t.regen,
                )
            })
            .collect();
        assert_eq!(
            turns,
            vec![
                ("a1", None, "u1", true, true),
                ("b1", None, "u1", false, true),
                ("c1", None, "u1e", false, false),
                ("a2", Some("a1"), "u2", true, false),
            ]
        );
        assert_eq!(g.turns[0].text, "first line\nsecond");
        assert_eq!(g.anchors.get("kid").map(|t| t.0.as_str()), Some("a1"));
        assert!(
            !g.anchors.contains_key("grandkid"),
            "only the main line's children"
        );
        assert_eq!(g.anchors.get("t1").map(|t| t.0.as_str()), Some("a1"));
        let kid = &g.branches[&"kid".into()];
        assert_eq!((kid.calls, kid.tools, kid.last_at), (1, 1, 25));
        assert_eq!(g.branches[&"grandkid".into()].calls, 1);
        assert_eq!(g.anchors.get("side").map(|t| t.0.as_str()), Some("a1"));
        assert_eq!(g.branches[&"side".into()].text.as_deref(), Some("why?"));
    }

    /// A message the head sits on stands as a turn of its own, beside the
    /// replies it already has (a regenerate under way).
    #[test]
    fn a_message_without_its_reply_on_the_line() {
        let nodes: HashMap<NodeId, Node> = [user("u1", None, 10, "hi"), reply("a1", "u1", 20, &[])]
            .into_iter()
            .map(|n| (n.id.clone(), n))
            .collect();
        let branches: HashMap<BranchId, Branch> =
            [branch("main", "u1", None, json!({ "type": "main" }))]
                .into_iter()
                .map(|b| (b.id.clone(), b))
                .collect();
        let g = build(&branches[&"main".into()], &nodes, &branches, &[]);
        let turns: Vec<(&str, bool, bool)> = g
            .turns
            .iter()
            .map(|t| (t.id.0.as_str(), t.line, t.regen))
            .collect();
        assert_eq!(turns, vec![("u1", true, true), ("a1", false, true)]);
    }
}
