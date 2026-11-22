// The trace view's read model: the active path's nodes in order, each
// sub-agent branch nested under the call that started it.

import { computed } from 'vue'
import type { Branch, Node, NodeId } from '../types'
import { WALK_LIMIT, pathNodes } from './derived'
import { state } from './state'

export type TraceRow =
  { kind: 'node'; key: string; node: Node } | { kind: 'agent'; key: string; branch: Branch; rows: TraceRow[] }

/** What the trace view can hide: node types, with markers (notes without
 *  text) apart from notes. */
export type TraceKind = 'user' | 'assistant' | 'tool' | 'note' | 'marker'

/** Nested sub-agents deeper than this are listed, not expanded. */
const MAX_DEPTH = 8

/** A branch's own nodes: head back to (not including) its fork point. */
function ownNodes(b: Branch): Node[] {
  const out: Node[] = []
  let cur: NodeId | null | undefined = b.head
  while (cur && cur !== b.forked_from && state.nodes[cur] && out.length < WALK_LIMIT) {
    const n: Node = state.nodes[cur]
    out.push(n)
    cur = n.parent
  }
  return out.reverse()
}

/** Sub-agent branches by the call that started them, oldest first. */
type Agents = Map<string, Branch[]>

function build(nodes: Node[], agents: Agents, depth: number): TraceRow[] {
  const answered = new Set<string>()
  for (const n of nodes) if (n.body.type === 'tool_result') answered.add(n.body.call_id)

  const rows: TraceRow[] = []
  const agentsOf = (callId: string): void => {
    for (const b of agents.get(callId) ?? []) {
      // Its nodes come when its row is opened (TraceView).
      const own = state.agentNodes[b.id] === 'loaded' ? ownNodes(b) : []
      const children = depth < MAX_DEPTH ? build(own, agents, depth + 1) : []
      rows.push({ kind: 'agent', key: `agent:${b.id}`, branch: b, rows: children })
    }
  }
  for (const n of nodes) {
    rows.push({ kind: 'node', key: n.id, node: n })
    // A finished sub-agent sits under its result; one still running, under
    // the call that started it (a direct call, or the model call whose
    // script made it).
    if (n.body.type === 'tool_result') agentsOf(n.body.call_id)
    else if (n.body.type === 'assistant') {
      for (const c of n.body.tool_calls) {
        if (!answered.has(c.call_id)) agentsOf(c.call_id)
        for (const [id] of agents) {
          if (id.startsWith(`${c.call_id}.`) && !answered.has(id)) agentsOf(id)
        }
      }
    }
  }
  return rows
}

/** The open session's trace, top to bottom. */
export const trace = computed<TraceRow[]>(() => {
  const agents: Agents = new Map()
  for (const b of Object.values(state.branches)) {
    if (b.kind.type !== 'agent') continue
    const list = agents.get(b.kind.call_id) ?? []
    list.push(b)
    agents.set(b.kind.call_id, list)
  }
  for (const list of agents.values()) list.sort((x, y) => x.created_at - y.created_at)
  return build(pathNodes.value, agents, 0)
})
