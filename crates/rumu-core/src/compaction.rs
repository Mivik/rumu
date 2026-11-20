//! Compaction: a note that stands in for the older part of a path.
//!
//! The note's text is a summary of the nodes before it; its [`Compaction`]
//! payload names the first node of the verbatim tail it keeps. Nothing is
//! removed from the tree: [`view`] is what the model sees, while the path's
//! state still folds every node.

use serde::{Deserialize, Serialize};

use crate::{Node, NodeBody, NodeId, Structured};

/// Why a compaction was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CompactReason {
    /// The view grew past the threshold.
    Auto,
    /// The user asked for it.
    Manual,
    /// The provider rejected a request as too long.
    Overflow,
}

/// Payload of the `branch.compaction` note (v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Compaction {
    /// First node of the verbatim tail; `None` keeps no tail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep_from: Option<NodeId>,
    /// First and last node the summary covers.
    pub from: NodeId,
    pub to: NodeId,
    /// Estimated view size before and after, tokens.
    pub before: u64,
    pub after: u64,
    pub reason: CompactReason,
    /// The model that wrote the summary.
    pub model: String,
}

impl Structured for Compaction {
    const TAG: &'static str = "branch.compaction";
    const V: u32 = 1;
}

/// The compaction payload of `node`, if it is a compaction note.
pub fn compaction_of(node: &Node) -> Option<Compaction> {
    match &node.body {
        NodeBody::Note { content, .. } => content.decode::<Compaction>(),
        _ => None,
    }
}

/// The latest compaction note's position and the first position of its
/// verbatim tail (the note itself when there is no tail).
fn compaction_cut(nodes: &[Node]) -> Option<(usize, usize)> {
    let (c, compaction) = nodes
        .iter()
        .enumerate()
        .rev()
        .find_map(|(i, n)| compaction_of(n).map(|x| (i, x)))?;
    let keep = compaction
        .keep_from
        .and_then(|k| nodes[..c].iter().position(|n| n.id == k))
        .unwrap_or(c);
    Some((c, keep))
}

/// Positions of `nodes` (root → head) the model sees, in projection order:
/// everything without a compaction; otherwise the last compaction note,
/// then its tail (`keep_from` up to the note, other compactions skipped),
/// then everything after it.
pub fn view_order(nodes: &[Node]) -> Vec<usize> {
    let Some((c, keep)) = compaction_cut(nodes) else {
        return (0..nodes.len()).collect();
    };
    std::iter::once(c)
        .chain((keep..c).filter(|&i| compaction_of(&nodes[i]).is_none()))
        .chain(c + 1..nodes.len())
        .collect()
}

/// The nodes of [`view_order`]. The compaction note stands before the kept
/// tail, so the thinking blocks of that tail were produced under a prefix the
/// model no longer sees: their signatures no longer verify against it (the
/// provider drops or rejects them). Clear them here, where history is
/// rewritten, per the replay rule; the path keeps them for display.
pub fn view(nodes: &[Node]) -> Vec<Node> {
    let cut = compaction_cut(nodes);
    view_order(nodes)
        .into_iter()
        .map(|i| {
            let mut node = nodes[i].clone();
            if let Some((c, keep)) = cut
                && (keep..c).contains(&i)
                && let NodeBody::Assistant { reasoning, .. } = &mut node.body
            {
                reasoning.clear();
            }
            node
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Content, ReasoningBlock, SessionId};

    fn node(id: &str, body: NodeBody) -> Node {
        Node {
            id: NodeId::from(id),
            parent: None,
            session: SessionId::new(),
            at: 0,
            dur_ms: None,
            body,
        }
    }

    fn user(id: &str) -> Node {
        node(
            id,
            NodeBody::User {
                content: Content::text(id),
            },
        )
    }

    fn compaction(id: &str, keep_from: Option<&str>) -> Node {
        let data = Compaction {
            keep_from: keep_from.map(NodeId::from),
            from: NodeId::from("a"),
            to: NodeId::from("a"),
            before: 0,
            after: 0,
            reason: CompactReason::Auto,
            model: "m".into(),
        };
        node(
            id,
            NodeBody::Note {
                source: "system".into(),
                content: Content::note_with("summary", &data),
            },
        )
    }

    fn reply(id: &str) -> Node {
        node(
            id,
            NodeBody::Assistant {
                content: Content::text("ok"),
                reasoning: vec![ReasoningBlock::Thinking {
                    text: "thinking".into(),
                    signature: Some("sig".into()),
                }],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![],
                model: "m".into(),
                usage: Default::default(),
            },
        )
    }

    fn ids(nodes: &[Node]) -> Vec<&str> {
        nodes.iter().map(|n| n.id.0.as_str()).collect()
    }

    #[test]
    fn no_compaction_is_the_whole_path() {
        let path = [user("a"), user("b")];
        assert_eq!(ids(&view(&path)), ["a", "b"]);
    }

    #[test]
    fn the_summary_leads_its_tail() {
        let path = [
            user("a"),
            user("b"),
            user("c"),
            compaction("x", Some("b")),
            user("d"),
        ];
        assert_eq!(ids(&view(&path)), ["x", "b", "c", "d"]);
        let path = [user("a"), compaction("x", None), user("d")];
        assert_eq!(ids(&view(&path)), ["x", "d"]);
    }

    #[test]
    fn only_the_last_compaction_counts() {
        let path = [
            user("a"),
            compaction("x", None),
            user("b"),
            user("c"),
            compaction("y", Some("x")),
            user("d"),
        ];
        assert_eq!(ids(&view(&path)), ["y", "b", "c", "d"]);
    }

    #[test]
    fn the_kept_tail_loses_its_reasoning() {
        let path = [
            user("a"),
            reply("r1"),
            user("b"),
            reply("r2"),
            compaction("x", Some("b")),
            reply("r3"),
        ];
        let seen = view(&path);
        assert_eq!(ids(&seen), ["x", "b", "r2", "r3"]);
        for n in &seen {
            let NodeBody::Assistant { reasoning, .. } = &n.body else {
                continue;
            };
            match n.id.0.as_str() {
                // The summary now leads this one: its prefix was rewritten.
                "r2" => assert!(reasoning.is_empty(), "kept-tail reasoning cleared"),
                // Produced after the compaction, under the current prefix.
                "r3" => assert!(!reasoning.is_empty(), "new reasoning kept"),
                other => panic!("unexpected assistant {other}"),
            }
        }
        // The path keeps the transcript for display.
        let NodeBody::Assistant { reasoning, .. } = &path[3].body else {
            panic!("r2 is an assistant");
        };
        assert!(!reasoning.is_empty(), "path keeps the reply's reasoning");
    }
}
