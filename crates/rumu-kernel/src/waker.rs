//! Wakers: what can still start a turn on a branch, or append to it, without
//! the user — a loop running there, a queued wake, a background task. Each
//! one hangs from a node of the main line (the assistant node holding the
//! call that started it); moving the main line off that node would leave it
//! reporting to a line that never made the call.

use std::collections::{HashMap, HashSet};

use rumu_core::{Branch, BranchId, BranchKind, Node, NodeBody, NodeId, TaskKind, Timestamp};

use crate::{Inner, Kernel, Result};

/// What keeps a branch live.
pub(crate) enum WakerKind {
    /// A loop runs on the branch.
    Loop,
    /// A wake item is queued and no call has seen it yet.
    Wake,
    /// A background task still runs, or its completion is not posted yet.
    Task {
        agent: bool,
        call_id: String,
        started_at: Timestamp,
        /// `bash: pnpm dev`, `agent: …`.
        label: String,
    },
}

pub(crate) struct Waker {
    /// The branch it wakes.
    pub branch: BranchId,
    pub kind: WakerKind,
}

/// Where the sub-agents of one main line hang from it.
pub(crate) struct Anchors {
    /// For each agent branch below the main line, the main-line node holding
    /// the `agent` call of its outermost ancestor.
    agents: HashMap<BranchId, Option<NodeId>>,
    /// Each agent branch's task, as a task label: `agent: …`.
    tasks: HashMap<BranchId, String>,
    /// The main line's assistant nodes by call id, oldest first.
    calls: HashMap<String, Vec<(Timestamp, NodeId)>>,
}

impl Anchors {
    /// The main-line node `w` hangs from, `None` when it hangs from none
    /// (the main line's own loop or wake, a branch this map has not seen).
    pub fn of(&self, main: &BranchId, w: &Waker) -> Option<&NodeId> {
        if &w.branch != main {
            return self.agents.get(&w.branch)?.as_ref();
        }
        match &w.kind {
            WakerKind::Task {
                call_id,
                started_at,
                ..
            } => self.call(call_id, *started_at),
            _ => None,
        }
    }

    /// How `w` reads in an error message.
    pub fn label(&self, w: &Waker) -> String {
        match &w.kind {
            WakerKind::Task { label, .. } => label.clone(),
            _ => self
                .tasks
                .get(&w.branch)
                .cloned()
                .unwrap_or_else(|| format!("branch {}", w.branch)),
        }
    }

    /// The latest main-line node holding `call_id` that is no newer than
    /// `before`: a provider may reuse call ids across variants, and the node
    /// a task or sub-agent came from was written just before it started.
    fn call(&self, call_id: &str, before: Timestamp) -> Option<&NodeId> {
        let nodes = self.calls.get(call_id)?;
        nodes
            .iter()
            .rev()
            .find(|(at, _)| *at <= before)
            .or(nodes.last())
            .map(|(_, id)| id)
    }
}

impl Kernel {
    /// Every waker of the branches in `subtree`; the caller holds `inner`.
    pub(crate) fn wakers_locked(inner: &Inner, subtree: &HashSet<BranchId>) -> Vec<Waker> {
        let mut out = Vec::new();
        for branch in subtree {
            if inner.running.contains_key(branch) {
                out.push(Waker {
                    branch: branch.clone(),
                    kind: WakerKind::Loop,
                });
            } else if inner.wake_pending.get(branch).is_some_and(|p| p.active) {
                out.push(Waker {
                    branch: branch.clone(),
                    kind: WakerKind::Wake,
                });
            }
        }
        for e in inner.tasks.values() {
            if subtree.contains(&e.info.branch) && (e.info.status.is_running() || !e.post_done) {
                out.push(Waker {
                    branch: e.info.branch.clone(),
                    kind: WakerKind::Task {
                        agent: matches!(e.info.kind, TaskKind::Agent { .. }),
                        call_id: e.info.call_id.clone(),
                        started_at: e.info.started_at,
                        label: e.info.label(),
                    },
                });
            }
        }
        out
    }

    /// Where the wakers below the main line `main` hang from it.
    pub(crate) fn anchors(&self, main: &Branch) -> Result<Anchors> {
        let nodes = self.store.nodes_of_session(&main.session)?;
        let branches = self.store.branches_of_session(&main.session)?;
        let off = Self::off_line_of(&nodes, &branches);
        let mut calls: HashMap<String, Vec<(Timestamp, NodeId)>> = HashMap::new();
        for n in nodes.iter().filter(|n| !off.contains(&n.id)) {
            if let NodeBody::Assistant { tool_calls, .. } = &n.body {
                for c in tool_calls {
                    calls
                        .entry(c.call_id.clone())
                        .or_default()
                        .push((n.at, n.id.clone()));
                }
            }
        }
        for list in calls.values_mut() {
            list.sort();
        }
        let mut anchors = Anchors {
            agents: HashMap::new(),
            tasks: HashMap::new(),
            calls,
        };
        let by_id: HashMap<&BranchId, &Branch> = branches.iter().map(|b| (&b.id, b)).collect();
        for b in &branches {
            if let BranchKind::Agent { task, .. } = &b.kind {
                let line = task.lines().next().unwrap_or_default();
                anchors.tasks.insert(b.id.clone(), format!("agent: {line}"));
            }
            // Up to the agent whose parent is the main line.
            let mut top = b;
            let anchor = loop {
                let BranchKind::Agent {
                    parent_branch,
                    call_id,
                    via_plugin,
                    ..
                } = &top.kind
                else {
                    break None;
                };
                if parent_branch == &main.id {
                    // A script's call is `"{model_call}.{seq}"`.
                    let call = match call_id.rsplit_once('.') {
                        Some((outer, _)) if *via_plugin => outer,
                        _ => call_id.as_str(),
                    };
                    break Some(anchors.call(call, top.created_at).cloned());
                }
                match by_id.get(parent_branch) {
                    Some(parent) => top = parent,
                    None => break None,
                }
            };
            if let Some(anchor) = anchor {
                anchors.agents.insert(b.id.clone(), anchor);
            }
        }
        Ok(anchors)
    }

    /// The nodes from the root down to `node`.
    pub(crate) fn ancestry(&self, node: &NodeId) -> Result<HashSet<NodeId>> {
        let mut out = HashSet::new();
        let mut cur = Some(node.clone());
        while let Some(id) = cur {
            cur = self.store.get_node(&id)?.and_then(|n| n.parent);
            out.insert(id);
        }
        Ok(out)
    }

    /// Whether the sub-agent `agent`, a child of the main line `main`, came
    /// from a call the main line no longer holds (it was moved off it). Its
    /// replies then stay on its own branch.
    pub(crate) fn detached(&self, agent: &BranchId, main: &Branch) -> Result<bool> {
        let anchors = self.anchors(main)?;
        let Some(Some(anchor)) = anchors.agents.get(agent) else {
            return Ok(false);
        };
        Ok(!self.ancestry(&main.head)?.contains(anchor))
    }

    /// The nodes its sub-agents and side questions added to a session: each
    /// such branch's own, from its head up to the node it forked from.
    pub(crate) fn off_line_of(nodes: &[Node], branches: &[Branch]) -> HashSet<NodeId> {
        let parent: HashMap<&NodeId, Option<&NodeId>> =
            nodes.iter().map(|n| (&n.id, n.parent.as_ref())).collect();
        let mut off = HashSet::new();
        for b in branches {
            if matches!(b.kind, BranchKind::Main) {
                continue;
            }
            let mut cur = Some(&b.head);
            while let Some(id) = cur.filter(|id| Some(*id) != b.forked_from.as_ref()) {
                off.insert(id.clone());
                cur = parent.get(id).copied().flatten();
            }
        }
        off
    }
}
