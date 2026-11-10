// Derived (read-only) views over the shared state.

import { computed } from 'vue'
import { rumu } from '../api'
import type { Branch, BranchId, Node, NodeId, SessionInfo, TaskInfo } from '../types'
import type { TurnError } from '../generated/TurnError'
import { TURN_ERROR_TAG } from '../types'
import type { DeltaBuffer, TurnCache } from './state'
import { state } from './state'

export const connected = rumu.connected

export const sortedSessions = computed(() => [...state.sessions].sort((a, b) => b.updated_at - a.updated_at))

/** Did the session change since the user last looked at it? */
export function isUnseen(s: SessionInfo): boolean {
  return s.seen_at != null && s.updated_at > s.seen_at
}

export const currentBranch = computed<Branch | null>(() =>
  state.activeBranch ? (state.branches[state.activeBranch] ?? null) : null,
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

/** root → head for the active branch. */
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

export const activeDelta = computed<DeltaBuffer | null>(() =>
  state.activeBranch ? (state.deltas[state.activeBranch] ?? null) : null,
)

export const isBusy = computed(() => !!state.activeBranch && !!state.busy[state.activeBranch])

export interface OwedReply {
  /** The recorded failure's message (empty when there is none) — then the
   * generic interruption text applies. */
  message: string
}

/** The active branch's unanswered failure, shown as the error row at the end
 * of the conversation while the branch is idle. Read from the path, so it
 * survives a reload: a `turn.error` marker behind the head's last real node
 * (a user message or tool result still owed its reply). The same rule the
 * server's `branch.retry` accepts. An answered or re-prompted failure is
 * history and stays quiet; a hook-refused prompt records nothing (a toast). */
export const owedReply = computed<OwedReply | null>(() => {
  const branch = state.activeBranch
  if (!branch || state.busy[branch]) return null
  let failure: OwedReply | null = null
  for (const n of [...pathNodes.value].reverse()) {
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

export interface SiblingAlternative {
  /** Branch to activate when it is picked. */
  branch: BranchId
  /** The active branch's head is inside this subtree. */
  current: boolean
}

/** parentId → its sibling alternatives, built in one pass over the branches
 * (each head walks up to the root once) so the per-message switcher is a
 * lookup, not another walk of every branch. */
const siblingIndex = computed<Map<NodeId, SiblingAlternative[]>>(() => {
  // Which child of each ancestor a non-agent branch's head descends through.
  const byChild = new Map<NodeId, Map<NodeId, Branch[]>>()
  for (const b of Object.values(state.branches)) {
    if (b.kind.type === 'agent') continue
    let cur: NodeId | null | undefined = b.head
    let guard = 0
    while (cur && state.nodes[cur] && guard++ < 10_000) {
      const parent = state.nodes[cur].parent
      if (parent) {
        let children = byChild.get(parent)
        if (!children) {
          children = new Map()
          byChild.set(parent, children)
        }
        const list = children.get(cur) ?? []
        list.push(b)
        children.set(cur, list)
      }
      cur = state.nodes[cur].parent
    }
  }
  // The child of each ancestor the active branch's head descends through.
  const activeChildOf = new Map<NodeId, NodeId>()
  const head = currentBranch.value?.head
  if (head) {
    let cur: NodeId | null | undefined = head
    let prev: NodeId | null = null
    let guard = 0
    while (cur && state.nodes[cur] && guard++ < 10_000) {
      if (prev) activeChildOf.set(cur, prev)
      prev = cur
      cur = state.nodes[cur].parent
    }
  }
  // The branch to activate for a child subtree: its most recent one.
  const pick = (branches: Branch[]): Branch =>
    branches.reduce((best, b) => (b.created_at > best.created_at ? b : best))
  const index = new Map<NodeId, SiblingAlternative[]>()
  for (const [parentId, children] of byChild) {
    if (children.size < 2) continue
    const activeChild = activeChildOf.get(parentId) ?? null
    index.set(
      parentId,
      [...children.entries()]
        .sort(([a], [b]) => (state.nodes[a]?.at ?? 0) - (state.nodes[b]?.at ?? 0) || a.localeCompare(b))
        .map(([nodeId, branches]) => ({
          branch: pick(branches).id,
          current: nodeId === activeChild,
        })),
    )
  }
  return index
})

/**
 * The alternatives at a message: one per child subtree of `parentId` that
 * holds a non-agent branch head. Subtrees that only lead to agent branches
 * (an `inherit: full` fork at an assistant node) do not.
 */
export function siblingAlternatives(parentId: NodeId | null): SiblingAlternative[] {
  if (!parentId) return []
  return siblingIndex.value.get(parentId) ?? []
}

/** Hook trace rows for the open session (unknown-branch rows are dropped here). */
export const hookTraceForSession = computed(() =>
  state.hookTrace.filter((e) => e.session !== null && e.session === state.sessionId),
)
