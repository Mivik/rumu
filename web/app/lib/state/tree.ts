// The open session's node tree and branches: loading, and moving the main
// line. Everything that replaces the tree goes through `replaceTree`,
// so the derived views always see a consistent snapshot.

import { rumu } from '../api'
import { t } from '../../i18n'
import type {
  Branch,
  BranchId,
  BranchPathResult,
  BranchWindowResult,
  Node,
  NodeId,
  PromptPreview,
  SessionId,
  SessionTreeResult,
  TurnCounts,
} from '../types'
import { state } from './state'
import { hasEarlier, pathNodes } from './derived'
import { syncLive } from './stream'
import { toast, withStop } from './ui'

/** Replace nodes/branches with an authoritative `session.tree`. */
export function replaceTree(tree: SessionTreeResult): void {
  state.session = tree.session
  state.nodes = {}
  state.branches = {}
  state.agentNodes = {}
  state.variantEnds = { ...tree.variant_ends }
  state.lineEnd = tree.end ?? null
  state.turnsBefore = {}
  state.turnsTotal = null
  state.seams = {}
  countTurns(tree)
  state.usage = tree.usage
  state.agentUsage = tree.agent_usage ?? {}
  for (const n of tree.nodes) state.nodes[n.id] = n
  markSeam(tree)
  for (const b of tree.branches) state.branches[b.id] = b
  syncLive(tree)
}

/** How much of the main line a session opens on, and each step back loads
 *  (nodes; widened to start at a user message). */
const TAIL = 400

/** The open session's tree: the main line's end (`TAIL` nodes; from node
 *  `from` on instead — where a reload left off) with the variants and side
 *  questions off it. What comes before or after comes as it is needed
 *  (`loadEarlier`, `loadLater`); sub-agents' nodes when one is opened
 *  (`ensureAgentNodes`). */
export function fetchTree(session: SessionId, from?: NodeId): Promise<SessionTreeResult> {
  return rumu.request<SessionTreeResult>('session.tree', { session, tail: TAIL, from })
}

/** What a window of the open session's tree brought; what is loaded stays. */
function merge(session: SessionId | null, res: BranchWindowResult): void {
  if (state.sessionId !== session) return
  const parent = res.nodes.find((n) => n.id === res.first)?.parent
  const joined = !parent || !!state.nodes[parent]
  for (const n of res.nodes) state.nodes[n.id] ??= n
  Object.assign(state.variantEnds, res.variant_ends)
  countTurns(res)
  if (!joined) markSeam(res)
}

/** A window that begins mid-turn, its start not loaded (one that goes on
 *  from what is loaded joins a turn, it does not split one). */
function markSeam(res: TurnCounts): void {
  const first = res.first ? state.nodes[res.first] : undefined
  if (!first?.parent || first.body.type === 'user' || state.seams[first.id] !== undefined) return
  state.seams[first.id] = Object.keys(state.seams).length
}

/** A window's turn counts: its first node has that many user messages
 *  before it, for good — its ancestors never change. */
function countTurns(res: TurnCounts): void {
  if (!res.first || res.turns_before === undefined) return
  state.turnsBefore[res.first] = res.turns_before
  state.turnsTotal = res.turns ?? null
}

let earlier: Promise<void> | null = null
let earlierAll = false

/**
 * Load what comes before the loaded part of the main line: one step back,
 * or (`all`) the rest of it — for the views that read the whole path
 * (stats, trace, an artifact's versions, a card's uses, a linked message).
 * One request at a time; resolves once the asked-for part is in.
 */
export async function loadEarlier(all = false): Promise<void> {
  while (earlier) {
    const running = earlier
    const covers = earlierAll || !all
    await running
    if (covers) return
  }
  const branch = state.mainBranch
  const first = pathNodes.value[0]
  if (!branch || !first?.parent) return
  const session = state.sessionId
  earlierAll = all
  earlier = rumu
    .request<BranchWindowResult>('branch.window', { branch, before: first.id, limit: all ? undefined : TAIL })
    .then((res) => merge(session, res))
    .catch((err: Error) => toast('error', t('errors.loadingBranch'), err.message))
    .finally(() => {
      earlier = null
    })
  await earlier
}

let later: Promise<void> | null = null
let laterAll = false

/**
 * Load what comes after the loaded part of the main line, when that stops
 * short of the head (`state.lineEnd`): one step on, or (`all`) the rest
 * of it. One request at a time; resolves once the asked-for part is in.
 */
export async function loadLater(all = false): Promise<void> {
  while (later) {
    const running = later
    const covers = laterAll || !all
    await running
    if (covers) return
  }
  const branch = state.mainBranch
  const after = state.lineEnd
  if (!branch || !after) return
  const session = state.sessionId
  laterAll = all
  later = rumu
    .request<BranchWindowResult>('branch.window', { branch, after, limit: all ? undefined : TAIL })
    .then((res) => {
      if (state.sessionId !== session || state.lineEnd !== after) return
      merge(session, res)
      state.lineEnd = res.end ?? null
      // what the head grew while the stretch was on its way came as
      // pushes it could not place yet
      const head = state.branches[branch]?.head
      if (!state.lineEnd && head && !state.nodes[head]) return loadHead(branch)
    })
    .catch((err: Error) => toast('error', t('errors.loadingBranch'), err.message))
    .finally(() => {
      later = null
    })
  await later
}

/** The whole main line, loaded. */
export async function ensureFullPath(): Promise<void> {
  if (state.lineEnd) await loadLater(true)
  if (hasEarlier.value) await loadEarlier(true)
}

/** The main line's head moved somewhere not loaded (a variant picked in
 *  another tab): load the end there, as an open does. */
export async function loadHead(branch: BranchId): Promise<void> {
  const session = state.sessionId
  try {
    merge(session, await rumu.request<BranchWindowResult>('branch.window', { branch, limit: TAIL }))
  } catch (err) {
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

/** Show the main line at its end again, from a stretch short of it: its
 *  end loads in place of what is shown, and the view opens there. */
export async function goToEnd(): Promise<void> {
  const branch = state.mainBranch
  if (!branch || !state.lineEnd) return
  const session = state.sessionId
  try {
    // the end may still be here from before (a jump to the start)
    const head = state.branches[branch]?.head
    if (!head || !state.nodes[head]) {
      const res = await rumu.request<BranchWindowResult>('branch.window', { branch, limit: TAIL })
      if (state.sessionId !== session) return
      merge(session, res)
    }
    state.lineEnd = null
    state.endJumps++
  } catch (err) {
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

/** The main line's prompts from turn `from` on, `limit` of them, as
 *  previews (nothing is loaded into the tree); null when the session or
 *  the line changed meanwhile, or the request failed. */
export async function fetchPrompts(from: number, limit: number): Promise<PromptPreview[] | null> {
  const branch = state.mainBranch
  if (!branch) return null
  const session = state.sessionId
  try {
    const res = await rumu.request<{ prompts: PromptPreview[] }>('branch.prompts', { branch, from, limit })
    return state.sessionId === session && state.mainBranch === branch ? res.prompts : null
  } catch {
    return null
  }
}

/** Show the main line from its start, from a stretch that does not reach
 *  back there: its start loads in place of what is shown (joined to it
 *  when they meet), and the view opens there. */
export function goToStart(): Promise<void> {
  return jumpLine({ start: true }, null)
}

/** Show the main line from its `n`th turn (1-based), not loaded: as
 *  `goToStart`, the view opening at that prompt. */
export function goToTurn(n: number): Promise<void> {
  return jumpLine({ turn: n }, n)
}

async function jumpLine(at: { start: true } | { turn: number }, turn: number | null): Promise<void> {
  const branch = state.mainBranch
  const first = pathNodes.value[0]
  if (!branch) return
  const session = state.sessionId
  try {
    const res = await rumu.request<BranchWindowResult>('branch.window', { branch, ...at, limit: TAIL })
    if (state.sessionId !== session) return
    merge(session, res)
    if (!first || !res.nodes.some((n) => n.id === first.id)) state.lineEnd = res.end ?? null
    // the window opens at the turn's user message (or the last one)
    const node = turn === null ? null : (res.first ?? null)
    state.lineJump = { seq: state.lineJump.seq + 1, node }
  } catch (err) {
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

/** Fetch the nodes of sub-agent branch `id` the tree left out, once per
 *  tree. A replaced tree drops them, so callers re-ask from a watcher. */
export async function ensureAgentNodes(id: BranchId): Promise<void> {
  if (state.branches[id]?.kind.type !== 'agent' || state.agentNodes[id]) return
  const session = state.sessionId
  state.agentNodes[id] = 'loading'
  try {
    const res = await rumu.request<{ nodes: Node[] }>('branch.nodes', { branch: id })
    // A tree replaced meanwhile asks again.
    if (state.sessionId !== session || state.agentNodes[id] !== 'loading') return
    for (const n of res.nodes) state.nodes[n.id] ??= n
    state.agentNodes[id] = 'loaded'
  } catch (err) {
    if (state.agentNodes[id] === 'loading') delete state.agentNodes[id]
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

export async function loadPath(branch: BranchId): Promise<void> {
  try {
    const res = await rumu.request<BranchPathResult>('branch.path', { branch })
    state.pathState = res.state
  } catch (err) {
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

/**
 * Move the session's main line to `node`: the variant switcher's pick (the
 * end of another variant), or just before a message being edited. What it
 * leaves stays reachable from the message's variant switcher. `label`
 * names the action in the error toast.
 */
export async function moveHead(
  node: NodeId,
  label: 'switch' | 'edit' = 'switch',
  fork?: NodeId,
): Promise<boolean> {
  const branch = state.session?.main_branch
  if (!branch) return false
  try {
    // The head moves by the `branch.updated` push, which comes first.
    const moved = await withStop((stop) => rumu.request('branch.move', { branch, node, stop }))
    if (moved === undefined) return false
    state.mainBranch = branch
    // Shown short of its end, the line goes on along the variant picked
    // after `fork` (the switcher's message): a stretch of it from there.
    // An edit stays put; the prompt it sends goes to the end.
    if (state.lineEnd && fork && label === 'switch') {
      const res = await rumu.request<BranchWindowResult>('branch.window', {
        branch,
        after: fork,
        limit: TAIL,
      })
      merge(state.sessionId, res)
      state.lineEnd = res.end ?? null
    }
    await loadPath(branch)
    return true
  } catch (err) {
    const message =
      label === 'edit' ? t('errors.action', { label: t('errors.labels.edit') }) : t('errors.switchVariant')
    toast('error', message, (err as Error).message)
    return false
  }
}

/**
 * The sub-agent branch spawned by the direct `agent` tool call `callId`
 * (recorded on the branch as `kind.call_id`). Prefers a child of the main
 * line (the common case); falls back to any match (a call nested in another
 * sub-agent).
 */
export function findAgentChild(callId: string): Branch | null {
  if (!callId) return null
  const matches = Object.values(state.branches).filter(
    (b): b is Branch => b.kind.type === 'agent' && !b.kind.via_plugin && b.kind.call_id === callId,
  )
  const isOwn = (b: Branch): boolean => b.kind.type === 'agent' && b.kind.parent_branch === state.mainBranch
  matches.sort((x, y) => Number(isOwn(y)) - Number(isOwn(x)))
  return matches[0] ?? null
}

/**
 * The sub-agent branches a `code` script forked on `branch` while its call
 * `codeCallId` runs: each records the script's inner call `"{codeCallId}.{n}"`.
 */
export function scriptAgents(branch: string, codeCallId: string): Branch[] {
  const prefix = `${codeCallId}.`
  return Object.values(state.branches)
    .filter(
      (b): b is Branch =>
        b.kind.type === 'agent' &&
        b.kind.via_plugin &&
        b.kind.parent_branch === branch &&
        b.kind.call_id.startsWith(prefix),
    )
    .sort((x, y) => x.created_at - y.created_at)
}
