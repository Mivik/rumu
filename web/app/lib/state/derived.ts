// Derived (read-only) views over the shared state.

import { computed } from 'vue'
import { rumu } from '../api'
import type { Branch, BranchId, Node, NodeId, TaskInfo } from '../types'
import type { DeltaBuffer, TurnCache } from './state'
import { state } from './state'

export const connected = rumu.connected

export const sortedSessions = computed(() => [...state.sessions].sort((a, b) => b.updated_at - a.updated_at))

export const currentBranch = computed<Branch | null>(() =>
  state.activeBranch ? (state.branches[state.activeBranch] ?? null) : null,
)

/** root → head for the active branch. */
export const pathNodes = computed<Node[]>(() => {
  const b = currentBranch.value
  if (!b) return []
  const out: Node[] = []
  let cur: NodeId | null | undefined = b.head
  let guard = 0
  while (cur && state.nodes[cur] && guard++ < 10_000) {
    const n: Node = state.nodes[cur]
    out.push(n)
    cur = n.parent
  }
  return out.reverse()
})

/** call_id → ToolCall, from assistant nodes on the path (for pairing results). */
export const callsOnPath = computed(() => {
  const map = new Map<string, { call: ToolCallLike; node: Node }>()
  for (const n of pathNodes.value) {
    if (n.body.type === 'assistant') {
      for (const c of n.body.tool_calls) map.set(c.call_id, { call: c, node: n })
    }
  }
  return map
})
type ToolCallLike = { call_id: string; name: string; args: unknown }

export const activeDelta = computed<DeltaBuffer | null>(() =>
  state.activeBranch ? (state.deltas[state.activeBranch] ?? null) : null,
)

export const isBusy = computed(() => !!state.activeBranch && !!state.busy[state.activeBranch])

export interface OwedReply {
  /** The live failure's message (empty when the server sent none) — then the
   * generic interruption text applies. */
  message: string
}

/** The failure the active branch hit during this page's lifetime, if any:
 * shown as the error row at the end of the conversation while the branch is
 * idle. Deliberately not derived from the tree — a stored `turn.error`
 * marker never brings the row back, so history stays quiet. A hook-refused
 * prompt (a blocked `turn.error`) never lands here either: it is a toast. */
export const owedReply = computed<OwedReply | null>(() => {
  const branch = state.activeBranch
  if (!branch || state.busy[branch]) return null
  const message = state.failedTurns[branch]
  return message === undefined ? null : { message }
})

/** Steering prompts waiting to be written on the active branch. */
export const queuedPrompts = computed(() =>
  state.activeBranch ? (state.queuedPrompts[state.activeBranch] ?? []) : [],
)

/** Tasks owned by the active branch, oldest first. */
export const branchTasks = computed<TaskInfo[]>(() => {
  const branch = state.activeBranch
  if (!branch) return []
  return Object.values(state.tasks)
    .filter((t) => t.branch === branch)
    .sort((a, b) => a.started_at - b.started_at)
})

/** Still-running tasks of the active branch. */
export const runningTasks = computed(() => branchTasks.value.filter((t) => t.status === 'running'))

/** Per-turn cache numbers for assistant nodes on the active path. */
export const cacheTurns = computed<TurnCache[]>(() => {
  let i = 0
  return pathNodes.value
    .filter((n) => n.body.type === 'assistant')
    .map((n) => {
      const b = n.body as Extract<Node['body'], { type: 'assistant' }>
      return {
        nodeId: n.id,
        index: ++i,
        prompt: b.usage.prompt_tokens,
        cached: b.usage.cached_tokens,
        creation: b.usage.cache_creation_tokens,
        completion: b.usage.completion_tokens,
      }
    })
})

/** Session-total cache numbers across ALL branches (whole tree). */
export const cacheTotal = computed(() => {
  let prompt = 0
  let cached = 0
  let creation = 0
  let completion = 0
  let turns = 0
  for (const n of Object.values(state.nodes)) {
    if (n.body.type !== 'assistant') continue
    turns += 1
    prompt += n.body.usage.prompt_tokens
    cached += n.body.usage.cached_tokens
    creation += n.body.usage.cache_creation_tokens
    completion += n.body.usage.completion_tokens
  }
  return { prompt, cached, creation, completion, turns }
})

/** Full root→head node id list per branch (used for fork-parent inference). */
const branchPaths = computed(() => {
  const map: Record<BranchId, NodeId[]> = {}
  for (const b of Object.values(state.branches)) {
    const ids: NodeId[] = []
    let cur: NodeId | null | undefined = b.head
    let guard = 0
    while (cur && state.nodes[cur] && guard++ < 10_000) {
      ids.push(cur)
      cur = state.nodes[cur].parent
    }
    map[b.id] = ids
  }
  return map
})

function parentBranchOf(b: Branch): BranchId | null {
  if (b.kind.type === 'main') return null
  if (b.kind.type === 'agent') return b.kind.parent_branch
  const ff = b.forked_from
  if (!ff) return null
  // The fork parent is the branch whose path contained `ff` when we forked.
  // Among current branches containing it, the shortest path is the closest.
  let best: Branch | null = null
  let bestLen = Infinity
  for (const other of Object.values(state.branches)) {
    if (other.id === b.id) continue
    const ids = branchPaths.value[other.id]
    if (!ids || !ids.includes(ff)) continue
    if (ids.length < bestLen) {
      best = other
      bestLen = ids.length
    }
  }
  return best ? best.id : null
}

export interface BranchTreeNode {
  branch: Branch
  children: BranchTreeNode[]
}

export const branchTree = computed<BranchTreeNode[]>(() => {
  const parentOf = new Map<BranchId, BranchId | null>()
  const kids = new Map<BranchId | null, Branch[]>()
  const all = Object.values(state.branches).sort((a, b) => a.created_at - b.created_at)
  for (const b of all) {
    const p = parentBranchOf(b)
    parentOf.set(b.id, p)
    if (!kids.has(p)) kids.set(p, [])
    kids.get(p)!.push(b)
  }
  const build = (parent: BranchId | null): BranchTreeNode[] =>
    (kids.get(parent) ?? []).map((branch) => ({ branch, children: build(branch.id) }))
  return build(null)
})

/** Hook trace rows for the open session (unknown-branch rows are dropped here). */
export const hookTraceForSession = computed(() =>
  state.hookTrace.filter((e) => e.session !== null && e.session === state.sessionId),
)
