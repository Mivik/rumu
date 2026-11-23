//! Side questions: the user's asides off a main line or a sub-agent. A side
//! branch forks at that line's last safe point, so asking never waits on a
//! running turn, and runs its own turns beside it. It sees the conversation up
//! to the fork; the line it left never sees it. It has no tools — the tool list stays in the
//! request (the prefix, and so the prompt cache, is the main line's), but
//! every call is refused.

use std::collections::HashSet;
use std::sync::Arc;

use rumu_core::{Branch, BranchId, BranchKind, Content, Node, NodeBody, Quote, SYSTEM_SOURCE};
use serde_json::json;

use crate::{Error, Kernel, Result};

/// The result of a tool call made on a side branch.
pub(crate) const SIDE_NO_TOOLS: &str = "side questions have no tools; answer from the conversation";

impl Kernel {
    /// `branch.side` WS method: a side question off `from` (the main line or
    /// a sub-agent), optionally about `quote`. Follow-ups are plain prompts
    /// on the branch.
    pub async fn side(
        self: &Arc<Self>,
        from: &BranchId,
        quote: Option<Quote>,
        content: Content,
    ) -> Result<Branch> {
        let parent = self
            .store
            .get_branch(from)?
            .ok_or_else(|| Error::BranchNotFound(from.clone()))?;
        if matches!(parent.kind, BranchKind::Side { .. }) {
            return Err(Error::NotMainLine(from.clone()));
        }
        let path = self.path_nodes(from)?;
        let fork = last_safe_node(&path)
            .ok_or_else(|| Error::Internal(format!("branch {from} has no root node")))?;
        let b = Self::make_branch(
            &parent.session,
            fork.id.clone(),
            BranchKind::Side {
                parent_branch: from.clone(),
                quote: quote.clone(),
            },
        );
        self.store.insert_branch(&b)?;
        self.push("branch.updated", json!({ "branch": b.clone() }));
        let note = NodeBody::Note {
            source: SYSTEM_SOURCE.into(),
            content: Content::text(side_note(quote.as_ref())),
        };
        self.append_body(&b.id, note, None).await?;
        self.prompt(&b.id, content).await?;
        Ok(b)
    }
}

/// The latest node of `path` with no tool call left unanswered: where a
/// branch can fork off a line that may be in the middle of a turn.
fn last_safe_node(path: &[Node]) -> Option<&Node> {
    let mut open = HashSet::new();
    let mut safe = None;
    for n in path {
        match &n.body {
            NodeBody::Assistant { tool_calls, .. } => {
                open.extend(tool_calls.iter().map(|c| c.call_id.as_str()));
            }
            NodeBody::ToolResult { call_id, .. } => {
                open.remove(call_id.as_str());
            }
            _ => {}
        }
        if open.is_empty() {
            safe = Some(n);
        }
    }
    safe
}

/// The framing a side branch opens with, ahead of the user's question.
fn side_note(quote: Option<&Quote>) -> String {
    let mut text = String::from(
        "The user is asking a side question. It branches off the conversation above: \
         the conversation will not see this exchange, and its work goes on without it. \
         Answer directly from what the conversation already holds. You have no tools \
         here; every call is refused. If the answer needs something you have not seen, \
         say so instead of offering to look.",
    );
    if let Some(q) = quote {
        text.push_str("\n\nThe question is about this passage of an earlier reply:\n");
        for line in q.text.lines() {
            text.push_str("> ");
            text.push_str(line);
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{CallStats, NodeId, SessionId, ToolCall, Usage};

    fn node(body: NodeBody) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: SessionId::new(),
            at: 0,
            dur_ms: None,
            body,
        }
    }

    fn calls(ids: &[&str]) -> NodeBody {
        NodeBody::Assistant {
            content: Content::default(),
            reasoning: vec![],
            stats: CallStats::default(),
            reasoning_reset: false,
            tool_calls: ids
                .iter()
                .map(|id| ToolCall::new(*id, "t", json!({})))
                .collect(),
            model: String::new(),
            usage: Usage::default(),
        }
    }

    fn result(id: &str) -> NodeBody {
        NodeBody::ToolResult {
            call_id: id.into(),
            content: Content::default(),
            is_error: false,
            invoke: None,
        }
    }

    #[test]
    fn a_line_mid_batch_forks_before_the_batch() {
        let user = node(NodeBody::User {
            content: Content::text("go"),
        });
        let mut path = vec![user.clone(), node(calls(&["a", "b"])), node(result("a"))];
        assert_eq!(last_safe_node(&path).map(|n| &n.id), Some(&user.id));
        path.push(node(result("b")));
        assert_eq!(last_safe_node(&path).map(|n| &n.id), Some(&path[3].id));
    }
}
