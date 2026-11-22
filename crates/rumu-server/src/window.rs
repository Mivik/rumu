//! What of a session's tree a client loads. The web opens a long session on
//! the end of its main line and fetches what came before as it needs it
//! (`session.tree { tail }`, `branch.window`); a sub-agent's own nodes come
//! when it is opened (`branch.nodes`).

use std::collections::{HashMap, HashSet};

use rumu_core::{Branch, BranchId, BranchKind, Node, NodeBody, NodeId};
use serde_json::Value;

/// The nodes the branches of a kind added: each one's own, head up to the
/// node it forked from.
fn own_nodes<'a>(
    nodes: &'a HashMap<NodeId, Node>,
    branches: &'a HashMap<BranchId, Branch>,
    kind: impl Fn(&BranchKind) -> bool,
) -> HashSet<&'a NodeId> {
    let mut out = HashSet::new();
    for b in branches.values().filter(|b| kind(&b.kind)) {
        let mut cur = nodes.get(&b.head);
        while let Some(n) = cur.filter(|n| Some(&n.id) != b.forked_from.as_ref()) {
            out.insert(&n.id);
            cur = n.parent.as_ref().and_then(|p| nodes.get(p));
        }
    }
    out
}

/// The nodes only sub-agents added, nested sub-agents included. The rest —
/// the main line with every variant it left, and the side questions — is
/// the session's line.
pub fn agent_nodes<'a>(
    nodes: &'a HashMap<NodeId, Node>,
    branches: &'a HashMap<BranchId, Branch>,
) -> HashSet<&'a NodeId> {
    own_nodes(nodes, branches, |k| matches!(k, BranchKind::Agent { .. }))
}

/// What a client loads of a stretch of a branch's path.
#[derive(Default)]
pub struct Window<'a> {
    /// The stretch, the side questions off it, and the first node of each
    /// variant off it; oldest first.
    pub nodes: Vec<&'a Node>,
    /// Each variant's newest end, by its first node (when that is not the
    /// end itself): where its switcher moves the head. The rest of a variant
    /// loads when it is switched to — an edited-away continuation can be
    /// most of a session.
    pub ends: HashMap<&'a NodeId, &'a NodeId>,
    /// The stretch's last node, when it stops short of the head.
    pub end: Option<&'a NodeId>,
    /// The stretch's first node, the user messages on the path before it,
    /// and on all of it: a client numbers the turns it shows from these.
    pub first: Option<&'a NodeId>,
    pub turns_before: usize,
    pub turns: usize,
}

/// A [`Window`] cloned out of the store, to serialize once it is let go.
#[derive(Default)]
pub struct Owned {
    pub nodes: Vec<Node>,
    pub ends: HashMap<NodeId, NodeId>,
    pub end: Option<NodeId>,
    pub first: Option<NodeId>,
    pub turns_before: usize,
    pub turns: usize,
}

impl Window<'_> {
    pub fn owned(self) -> Owned {
        Owned {
            nodes: self.nodes.into_iter().cloned().collect(),
            ends: self
                .ends
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            end: self.end.cloned(),
            first: self.first.cloned(),
            turns_before: self.turns_before,
            turns: self.turns,
        }
    }
}

impl Owned {
    /// The nodes as a client gets them ([`wire`]).
    pub fn wire_nodes(&self) -> Vec<Value> {
        self.nodes.iter().map(wire).collect()
    }
}

/// Which stretch of a path a window is; `limit`s count nodes of the path.
#[derive(Clone, Copy)]
pub enum Span<'a> {
    /// Its end: the last `limit` nodes (all of them without).
    Tail(Option<usize>),
    /// The last `limit` nodes before a node (all of them without).
    Before(&'a NodeId, Option<usize>),
    /// `limit` nodes from a node on: where a reader left off.
    From(&'a NodeId, usize),
    /// `limit` nodes after a node.
    After(&'a NodeId, usize),
    /// Its start: the first `limit` nodes.
    Start(usize),
    /// `limit` nodes from its `n`th user message on (1-based; past the
    /// last, the last).
    Turn(usize, usize),
}

/// A stretch of `head`'s path, whole turns of it: it starts at a user
/// message and stops before one (or at the head) — a turn split across
/// stretches would split its entry in the web when the rest arrives.
/// `None` when the span's node is not on the path.
pub fn window<'a>(
    nodes: &'a HashMap<NodeId, Node>,
    branches: &'a HashMap<BranchId, Branch>,
    head: &NodeId,
    span: Span,
) -> Option<Window<'a>> {
    let mut path = Vec::new();
    let mut cur = nodes.get(head);
    while let Some(n) = cur {
        path.push(n);
        cur = n.parent.as_ref().and_then(|p| nodes.get(p));
    }
    path.reverse();
    let len = path.len();
    let at = |id: &NodeId| path.iter().position(|n| &n.id == id);
    let is_user = |i: usize| matches!(path[i].body, NodeBody::User { .. });
    let (mut start, mut end) = match span {
        Span::Tail(limit) => (limit.map_or(0, |l| len.saturating_sub(l)), len),
        Span::Before(id, limit) => {
            let end = at(id)?;
            (limit.map_or(0, |l| end.saturating_sub(l)), end)
        }
        Span::From(id, limit) => {
            let start = at(id)?;
            // a place near the end loads the end, as much as the tail would
            let end = (start + limit).min(len);
            (start.min(len.saturating_sub(limit)), end)
        }
        Span::After(id, limit) => {
            let start = at(id)? + 1;
            (start, (start + limit).min(len))
        }
        Span::Start(limit) => (0, limit.min(len)),
        Span::Turn(n, limit) => {
            let users = (0..len).filter(|&i| is_user(i));
            let start = users.take(n.max(1)).last().unwrap_or(0);
            (start, (start + limit).min(len))
        }
    };
    while start > 0 && start < len && !is_user(start) {
        start -= 1;
    }
    while end < len && !is_user(end) {
        end += 1;
    }

    let on_path: HashSet<&NodeId> = path.iter().map(|n| &n.id).collect();
    let agents = agent_nodes(nodes, branches);
    let sides = own_nodes(nodes, branches, |k| matches!(k, BranchKind::Side { .. }));
    let mut off: HashMap<&NodeId, Vec<&Node>> = HashMap::new();
    for n in nodes.values() {
        if on_path.contains(&n.id) || agents.contains(&n.id) {
            continue;
        }
        if let Some(p) = &n.parent {
            off.entry(p).or_default().push(n);
        }
    }
    let kids = |id: &NodeId| off.get(id).into_iter().flatten().copied();

    let mut w = Window {
        nodes: path[start..end].to_vec(),
        ends: HashMap::new(),
        end: (end < len).then(|| &path[end - 1].id),
        first: path.get(start).filter(|_| start < end).map(|n| &n.id),
        turns_before: (0..start).filter(|&i| is_user(i)).count(),
        turns: (0..len).filter(|&i| is_user(i)).count(),
    };
    for &at in &path[start..end] {
        for child in kids(&at.id) {
            w.nodes.push(child);
            let mut stack = vec![child];
            if sides.contains(&child.id) {
                // a side question: all of it
                while let Some(n) = stack.pop() {
                    for k in kids(&n.id) {
                        w.nodes.push(k);
                        stack.push(k);
                    }
                }
                continue;
            }
            // a variant: its newest end (side questions off it are not
            // part of it)
            let mut newest = child;
            while let Some(n) = stack.pop() {
                let mut leaf = true;
                for k in kids(&n.id).filter(|k| !sides.contains(&k.id)) {
                    leaf = false;
                    stack.push(k);
                }
                if leaf && n.at > newest.at {
                    newest = n;
                }
            }
            if newest.id != child.id {
                w.ends.insert(&child.id, &newest.id);
            }
        }
    }
    w.nodes
        .sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
    Some(w)
}

/// The user messages on `head`'s path from the `from`th on (1-based),
/// `limit` of them, each with its turn number; and how many the path has.
/// What the web's rail wheel lists of turns it has not loaded.
pub fn prompts<'a>(
    nodes: &'a HashMap<NodeId, Node>,
    head: &NodeId,
    from: usize,
    limit: usize,
) -> (Vec<(usize, &'a Node)>, usize) {
    let mut users = Vec::new();
    let mut cur = nodes.get(head);
    while let Some(n) = cur {
        if matches!(n.body, NodeBody::User { .. }) {
            users.push(n);
        }
        cur = n.parent.as_ref().and_then(|p| nodes.get(p));
    }
    users.reverse();
    let total = users.len();
    let start = from.max(1) - 1;
    let got = users
        .into_iter()
        .enumerate()
        .skip(start)
        .take(limit)
        .map(|(i, n)| (i + 1, n))
        .collect();
    (got, total)
}

/// A prompt's opening for a one-line label: its text cut to `chars`
/// characters, with its attachments (references, no bytes).
pub fn prompt_preview(node: &Node, chars: usize) -> Value {
    let NodeBody::User { content } = &node.body else {
        return Value::Null;
    };
    let text: String = content.text.chars().take(chars).collect();
    serde_json::json!({ "text": text, "attachments": content.attachments })
}

/// `node` as a client gets it: without what only the model reads — thinking
/// signatures and redacted thinking's payload, opaque tokens the provider
/// replays. They are a tenth to a quarter of a long session's bytes and are
/// never shown.
pub fn wire(node: &Node) -> Value {
    let mut v = serde_json::to_value(node).expect("a node serializes");
    slim(&mut v);
    v
}

/// [`wire`] on a node already serialized (a `node.appended` push's).
pub fn slim(node: &mut Value) {
    let Some(blocks) = node
        .pointer_mut("/body/reasoning")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for b in blocks.iter_mut().filter_map(Value::as_object_mut) {
        b.remove("signature");
        if let Some(data) = b.get_mut("data") {
            *data = Value::Null;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{BranchStatus, Content};

    fn user(id: &str, parent: Option<&str>, at: i64) -> Node {
        Node {
            id: id.into(),
            parent: parent.map(Into::into),
            session: "s".into(),
            at,
            dur_ms: None,
            body: NodeBody::User {
                content: Content::text(id),
            },
        }
    }

    fn note(id: &str, parent: &str, at: i64) -> Node {
        Node {
            body: NodeBody::Note {
                content: Content::text(id),
                source: String::new(),
            },
            ..user(id, Some(parent), at)
        }
    }

    fn branch(id: &str, head: &str, forked_from: Option<&str>, kind: BranchKind) -> Branch {
        Branch {
            id: id.into(),
            session: "s".into(),
            head: head.into(),
            forked_from: forked_from.map(Into::into),
            kind,
            status: BranchStatus::Active,
            created_at: 0,
        }
    }

    fn agent(parent: &str) -> BranchKind {
        BranchKind::Agent {
            task: String::new(),
            parent_branch: parent.into(),
            call_id: "c".into(),
            via_plugin: false,
            inherit: Default::default(),
        }
    }

    fn ids(v: &[&Node]) -> Vec<String> {
        v.iter().map(|n| n.id.to_string()).collect()
    }

    /// r ─ u1 ─ a ─ u2 ─ b ─ u3 ─ c (main)
    ///          │    └ v (a variant of u2) ─ v2
    ///          ├ q (a side question off a)
    ///          └ g1 ─ g2 (a sub-agent forked at a) ─ n1 (its own sub-agent)
    fn tree() -> (HashMap<NodeId, Node>, HashMap<BranchId, Branch>) {
        let nodes = [
            note("r", "", 0),
            user("u1", Some("r"), 1),
            note("a", "u1", 2),
            user("u2", Some("a"), 3),
            note("b", "u2", 4),
            user("u3", Some("b"), 5),
            note("c", "u3", 6),
            user("v", Some("a"), 7),
            note("v2", "v", 8),
            note("q", "a", 9),
            note("g1", "a", 10),
            note("g2", "g1", 11),
            note("n1", "g2", 12),
        ];
        let mut nodes: HashMap<NodeId, Node> =
            nodes.into_iter().map(|n| (n.id.clone(), n)).collect();
        nodes.get_mut(&NodeId::from("r")).unwrap().parent = None;
        let side = BranchKind::Side {
            parent_branch: "main".into(),
            quote: None,
        };
        let branches = [
            branch("main", "c", None, BranchKind::Main),
            branch("side", "q", Some("a"), side),
            branch("g", "g2", Some("a"), agent("main")),
            branch("n", "n1", Some("g2"), agent("g")),
        ];
        (
            nodes,
            branches.into_iter().map(|b| (b.id.clone(), b)).collect(),
        )
    }

    #[test]
    fn agent_nodes_are_only_what_sub_agents_added() {
        let (nodes, branches) = tree();
        let mut got: Vec<String> = agent_nodes(&nodes, &branches)
            .into_iter()
            .map(|id| id.to_string())
            .collect();
        got.sort();
        assert_eq!(got, ["g1", "g2", "n1"]);
    }

    /// A variant comes as its first node and its newest end; a side
    /// question whole.
    #[test]
    fn whole_window_is_the_line_with_variant_stubs() {
        let (nodes, branches) = tree();
        let all = window(&nodes, &branches, &"c".into(), Span::Tail(None)).unwrap();
        assert_eq!(
            ids(&all.nodes),
            ["r", "u1", "a", "u2", "b", "u3", "c", "v", "q"]
        );
        let ends: Vec<_> = all
            .ends
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(ends, [("v".to_owned(), "v2".to_owned())]);
        assert_eq!(all.end, None);
    }

    /// The tail starts at a user message, and brings only what hangs off it.
    #[test]
    fn tail_starts_at_a_user_message() {
        let (nodes, branches) = tree();
        let tail = window(&nodes, &branches, &"c".into(), Span::Tail(Some(1))).unwrap();
        assert_eq!(ids(&tail.nodes), ["u3", "c"]);
        // two back from the head lands on `b`, mid-turn: widened to `u2`
        let tail = window(&nodes, &branches, &"c".into(), Span::Tail(Some(3))).unwrap();
        assert_eq!(ids(&tail.nodes), ["u2", "b", "u3", "c"]);
        assert!(tail.ends.is_empty());
    }

    /// What came before a node: its variants and side questions come with
    /// the stretch they hang from.
    #[test]
    fn earlier_stretch_brings_what_hangs_off_it() {
        let (nodes, branches) = tree();
        let u2 = NodeId::from("u2");
        let before = window(&nodes, &branches, &"c".into(), Span::Before(&u2, Some(2))).unwrap();
        assert_eq!(ids(&before.nodes), ["u1", "a", "v", "q"]);
        assert_eq!(before.end, Some(&"a".into()));
        let v = NodeId::from("v");
        assert!(window(&nodes, &branches, &"c".into(), Span::Before(&v, None)).is_none());
    }

    /// From a reader's place: whole turns, stopping short of the head (and
    /// saying where); near the end, the end.
    #[test]
    fn from_a_place_and_after_it() {
        let (nodes, branches) = tree();
        let head = NodeId::from("c");
        let (a, b, v) = (NodeId::from("a"), NodeId::from("b"), NodeId::from("v"));
        let w = window(&nodes, &branches, &head, Span::From(&a, 2)).unwrap();
        assert_eq!(ids(&w.nodes), ["u1", "a", "u2", "b", "v", "q"]);
        assert_eq!(w.end, Some(&b));
        let w = window(&nodes, &branches, &head, Span::From(&b, 3)).unwrap();
        assert_eq!(ids(&w.nodes), ["u2", "b", "u3", "c"]);
        assert_eq!(w.end, None);
        assert!(window(&nodes, &branches, &head, Span::From(&v, 3)).is_none());
        // what comes after a stretch, on to the next turn's start
        let w = window(&nodes, &branches, &head, Span::After(&a, 1)).unwrap();
        assert_eq!(ids(&w.nodes), ["u2", "b"]);
        assert_eq!(w.end, Some(&b));
        let w = window(&nodes, &branches, &head, Span::After(&b, 1)).unwrap();
        assert_eq!(ids(&w.nodes), ["u3", "c"]);
        assert_eq!(w.end, None);
        // its start, on to the next turn's start
        let w = window(&nodes, &branches, &head, Span::Start(2)).unwrap();
        assert_eq!(ids(&w.nodes), ["r", "u1", "a", "v", "q"]);
        assert_eq!(w.end, Some(&a));
        // prompts by turn number, along the path only
        let (got, total) = prompts(&nodes, &head, 2, 5);
        assert_eq!(total, 3);
        let got: Vec<_> = got.iter().map(|(k, n)| (*k, n.id.to_string())).collect();
        assert_eq!(got, [(2, "u2".to_string()), (3, "u3".to_string())]);
        // a turn by its number, counted along the path
        let w = window(&nodes, &branches, &head, Span::Turn(2, 1)).unwrap();
        assert_eq!(ids(&w.nodes), ["u2", "b"]);
        assert_eq!(
            (w.first, w.turns_before, w.turns),
            (Some(&"u2".into()), 1, 3)
        );
        let w = window(&nodes, &branches, &head, Span::Turn(9, 1)).unwrap();
        assert_eq!(ids(&w.nodes), ["u3", "c"]);
    }

    #[test]
    fn slim_drops_opaque_reasoning() {
        let mut v = serde_json::json!({ "body": { "type": "assistant", "reasoning": [
            { "type": "thinking", "text": "hmm", "signature": "sig" },
            { "type": "redacted", "data": "opaque" },
        ] } });
        slim(&mut v);
        let blocks = v["body"]["reasoning"].as_array().unwrap();
        assert_eq!(blocks[0]["text"], "hmm");
        assert!(blocks[0].get("signature").is_none());
        assert_eq!(blocks[1]["type"], "redacted");
        assert!(blocks[1]["data"].is_null());
    }
}
