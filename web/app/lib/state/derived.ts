// Derived (read-only) views over the shared state.

import { computed } from 'vue'
import { rumu } from '../api'
import type { Branch, BranchId, Node, NodeId, SessionInfo, TaskInfo } from '../types'
import type { SessionStatus } from '../generated/SessionStatus'
import type { TurnError } from '../generated/TurnError'
import { TURN_ERROR_TAG } from '../types'
import type { DeltaBuffer } from './state'
import { state } from './state'

export const connected = rumu.connected

export const sortedSessions = computed(() => [...state.sessions].sort((a, b) => b.touched_at - a.touched_at))

/** Did the session change since the user last looked at it? */
export function isUnseen(s: SessionInfo): boolean {
  return s.seen_at != null && s.updated_at > s.seen_at
}

/** What the session is doing, as the server last said (`session.updated`
 *  reaches every client, whatever session it watches). */
export function sessionStatus(s: SessionInfo): SessionStatus {
  return s.status ?? 'idle'
}

export const currentBranch = computed<Branch | null>(() =>
  state.mainBranch ? (state.branches[state.mainBranch] ?? null) : null,
)

/** root → head of `b`; with `stop`, only the nodes after that one. */
export function pathOf(b: Branch | null | undefined, stop?: NodeId | null): Node[] {
  if (!b) return []
  const out: Node[] = []
  let cur: NodeId | null | undefined = b.head
  let guard = 0
  while (cur && cur !== stop && state.nodes[cur] && guard++ < 10_000) {
    const n: Node = state.nodes[cur]
    out.push(n)
    cur = n.parent
  }
  return out.reverse()
}

/** root → head for the main line. */
export const pathNodes = computed<Node[]>(() => pathOf(currentBranch.value))

/** call_id → ToolCall, from the assistant nodes in `nodes` (for pairing results). */
export function callsOf(nodes: Node[]): Map<string, { call: ToolCallLike; node: Node }> {
  const map = new Map<string, { call: ToolCallLike; node: Node }>()
  for (const n of nodes) {
    if (n.body.type === 'assistant') {
      for (const c of n.body.tool_calls) map.set(c.call_id, { call: c, node: n })
    }
  }
  return map
}

export const callsOnPath = computed(() => callsOf(pathNodes.value))
type ToolCallLike = { call_id: string; name: string; args: unknown }

/** Every call in the loaded tree, by call_id. */
const callsInTree = computed(() => {
  const map = new Map<string, ToolCallLike>()
  for (const n of Object.values(state.nodes)) {
    if (n.body.type !== 'assistant') continue
    for (const c of n.body.tool_calls) map.set(c.call_id, c)
  }
  return map
})

export function callById(call_id: string): ToolCallLike | null {
  return callsInTree.value.get(call_id) ?? null
}

export const mainDelta = computed<DeltaBuffer | null>(() =>
  state.mainBranch ? (state.deltas[state.mainBranch] ?? null) : null,
)

export const isBusy = computed(() => !!state.mainBranch && !!state.busy[state.mainBranch])

export interface OwedReply {
  /** The recorded failure's message (empty when there is none) — then the
   * generic interruption text applies. */
  message: string
}

/** `branch`'s unanswered failure (`path` its nodes), shown as the error row at the end
 * of the conversation while the branch is idle. Read from the path, so it
 * survives a reload: a `turn.error` marker behind the head's last real node
 * (a user message or tool result still owed its reply). The same rule the
 * server's `branch.retry` accepts. An answered or re-prompted failure is
 * history and stays quiet; a hook-refused prompt records nothing (a toast). */
export function owedOf(branch: BranchId | null, path: Node[]): OwedReply | null {
  if (!branch || state.busy[branch]) return null
  let failure: OwedReply | null = null
  for (const n of [...path].reverse()) {
    if (n.body.type !== 'note') {
      const owes = n.body.type === 'user' || n.body.type === 'tool_result'
      return owes ? failure : null
    }
    const data = n.body.content.data
    if (!failure && data?.tag === TURN_ERROR_TAG) {
      failure = { message: (data.data as TurnError | null)?.message ?? '' }
    }
  }
  return null
}

/** Still-running tasks owned by `branch`, oldest first. */
export function runningOf(branch: BranchId | null): TaskInfo[] {
  if (!branch) return []
  return Object.values(state.tasks)
    .filter((t) => t.branch === branch && t.status === 'running')
    .sort((a, b) => a.started_at - b.started_at)
}

/** How many tasks the sub-agents of `root` (at any depth) are still
 *  running: no row shows them, but "stop all" takes them down too. */
export function nestedRunningOf(root: BranchId | null): number {
  if (!root) return 0
  const below = (id: BranchId): boolean => {
    for (let b = state.branches[id], guard = 0; b && guard++ < 1000;) {
      if (b.kind.type !== 'agent') return false
      if (b.kind.parent_branch === root) return true
      b = state.branches[b.kind.parent_branch]
    }
    return false
  }
  return Object.values(state.tasks).filter((t) => t.status === 'running' && below(t.branch)).length
}

export interface SiblingAlternative {
  /** Where the main line moves when it is picked: the newest end of its
   *  subtree. */
  node: NodeId
  /** The shown branch's head is inside this subtree. */
  current: boolean
}

/** Nodes a sub-agent or side question added (each such branch's own, head
 *  up to the node it forked from, as far as they are loaded): not part of
 *  the main line, so never a variant of it. */
const offLine = computed<Set<NodeId>>(() => {
  const off = new Set<NodeId>()
  for (const b of Object.values(state.branches)) {
    if (b.kind.type === 'main') continue
    let cur: NodeId | null | undefined = b.head
    let guard = 0
    while (cur && cur !== b.forked_from && state.nodes[cur] && guard++ < 10_000) {
      off.add(cur)
      cur = state.nodes[cur].parent
    }
  }
  return off
})

/** parentId → its sibling alternatives: the children of each main-line node
 * that has several, each with the newest leaf below it, built in one pass
 * (each leaf walks up only until an ancestor already has a newer one) so the
 * per-message switcher is a lookup. */
const siblingIndex = computed<Map<NodeId, SiblingAlternative[]>>(() => {
  const children = new Map<NodeId, Node[]>()
  for (const n of Object.values(state.nodes)) {
    if (!n.parent || offLine.value.has(n.id)) continue
    const list = children.get(n.parent) ?? []
    list.push(n)
    children.set(n.parent, list)
  }
  // The newest leaf under each node.
  const newest = new Map<NodeId, Node>()
  for (const leaf of Object.values(state.nodes)) {
    if (children.has(leaf.id) || offLine.value.has(leaf.id)) continue
    let cur: Node | undefined = leaf
    let guard = 0
    while (cur && guard++ < 10_000) {
      const best = newest.get(cur.id)
      if (best && best.at >= leaf.at) break
      newest.set(cur.id, leaf)
      cur = cur.parent ? state.nodes[cur.parent] : undefined
    }
  }
  // The child of each ancestor the shown head descends through.
  const shownChildOf = new Map<NodeId, NodeId>()
  let cur: NodeId | null | undefined = currentBranch.value?.head
  let prev: NodeId | null = null
  let guard = 0
  while (cur && state.nodes[cur] && guard++ < 10_000) {
    if (prev) shownChildOf.set(cur, prev)
    prev = cur
    cur = state.nodes[cur].parent
  }
  const index = new Map<NodeId, SiblingAlternative[]>()
  for (const [parentId, kids] of children) {
    if (kids.length < 2) continue
    const shown = shownChildOf.get(parentId) ?? null
    index.set(
      parentId,
      kids
        .sort((a, b) => a.at - b.at || a.id.localeCompare(b.id))
        .map((n) => ({ node: newest.get(n.id)?.id ?? n.id, current: n.id === shown })),
    )
  }
  return index
})

/**
 * The alternatives at a message: one per child subtree of `parentId` on the
 * main line. What sub-agents (a `mode: "fork"` child at an assistant node)
 * and side questions added is not one.
 */
export function siblingAlternatives(parentId: NodeId | null): SiblingAlternative[] {
  if (!parentId) return []
  return siblingIndex.value.get(parentId) ?? []
}
