// The trace view's read model: the active path's nodes in order, each
// sub-agent branch nested under the call that started it, and the hook
// firings seen since this page loaded interleaved by time.
//
// Hooks are not persisted (a reload starts with none) and carry the branch
// they fired on, so a list takes only its own branch's firings: a variant's
// hooks never interleave into another variant's path. The main list also
// takes firings whose branch the tree does not know yet (a sub-agent that
// just started). They sort by arrival time against the nodes' `at` (end
// time), so the order is close to the real order, but not exact.

import { computed } from 'vue'
import type { Branch, BranchId, Node, NodeId } from '../types'
import { hookTraceForSession, pathNodes } from './derived'
import type { HookTraceEntry } from './state'
import { state } from './state'

export type TraceRow =
  | { kind: 'node'; key: string; node: Node }
  | { kind: 'hook'; key: string; hook: HookTraceEntry }
  | { kind: 'agent'; key: string; branch: Branch; rows: TraceRow[] }

/** What the trace view can hide: node types, with markers (notes without
 *  text) apart from notes, and hook firings. */
export type TraceKind = 'user' | 'assistant' | 'tool' | 'note' | 'marker' | 'hook'

/** Nested sub-agents deeper than this are listed, not expanded. */
const MAX_DEPTH = 8

/** A branch's own nodes: head back to (not including) its fork point. */
function ownNodes(b: Branch): Node[] {
  const out: Node[] = []
  let cur: NodeId | null | undefined = b.head
  while (cur && cur !== b.forked_from && state.nodes[cur] && out.length < 10_000) {
    const n: Node = state.nodes[cur]
    out.push(n)
    cur = n.parent
  }
  return out.reverse()
}

interface Index {
  /** Sub-agent branches by the call that started them, oldest first. */
  agents: Map<string, Branch[]>
  hooks: Map<BranchId, HookTraceEntry[]>
}

function build(nodes: Node[], hooks: HookTraceEntry[], index: Index, depth: number): TraceRow[] {
  const answered = new Set<string>()
  for (const n of nodes) if (n.body.type === 'tool_result') answered.add(n.body.call_id)

  const rows: TraceRow[] = []
  let h = 0
  const agentsOf = (callId: string): void => {
    for (const b of index.agents.get(callId) ?? []) {
      // Its nodes come when its row is opened (TraceView).
      const own = state.agentNodes[b.id] === 'loaded' ? ownNodes(b) : []
      const children = depth < MAX_DEPTH ? build(own, index.hooks.get(b.id) ?? [], index, depth + 1) : []
      rows.push({ kind: 'agent', key: `agent:${b.id}`, branch: b, rows: children })
    }
  }
  for (const n of nodes) {
    while (h < hooks.length && hooks[h].at < n.at) {
      rows.push({ kind: 'hook', key: `hook:${hooks[h].seq}`, hook: hooks[h] })
      h++
    }
    rows.push({ kind: 'node', key: n.id, node: n })
    // A finished sub-agent sits under its result; one still running, under
    // the call that started it (a direct call, or the model call whose
    // script made it).
    if (n.body.type === 'tool_result') agentsOf(n.body.call_id)
    else if (n.body.type === 'assistant') {
      for (const c of n.body.tool_calls) {
        if (!answered.has(c.call_id)) agentsOf(c.call_id)
        for (const [id] of index.agents) {
          if (id.startsWith(`${c.call_id}.`) && !answered.has(id)) agentsOf(id)
        }
      }
    }
  }
  for (; h < hooks.length; h++) rows.push({ kind: 'hook', key: `hook:${hooks[h].seq}`, hook: hooks[h] })
  return rows
}

/** The open session's trace, top to bottom. */
export const trace = computed<TraceRow[]>(() => {
  const agents = new Map<string, Branch[]>()
  for (const b of Object.values(state.branches)) {
    if (b.kind.type !== 'agent') continue
    const list = agents.get(b.kind.call_id) ?? []
    list.push(b)
    agents.set(b.kind.call_id, list)
  }
  for (const list of agents.values()) list.sort((x, y) => x.created_at - y.created_at)

  const hooks = new Map<BranchId, HookTraceEntry[]>()
  const main: HookTraceEntry[] = []
  for (const e of hookTraceForSession.value) {
    if (e.branch === state.activeBranch || !state.branches[e.branch]) main.push(e)
    else {
      const list = hooks.get(e.branch) ?? []
      list.push(e)
      hooks.set(e.branch, list)
    }
  }
  return build(pathNodes.value, main, { agents, hooks }, 0)
})
