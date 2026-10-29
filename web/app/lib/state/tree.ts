// The open session's node tree and branches: loading, switching, forking and
// regenerating. Everything that replaces the tree goes through `replaceTree`,
// so the derived views always see a consistent snapshot.

import { rumu } from '../api'
import { t } from '../../i18n'
import type { MessageKey } from '../../i18n'
import type { Branch, BranchId, CodeCall, Node, NodeId, PathState, SessionTreeResult } from '../types'
import { state } from './state'
import { toast } from './ui'

/** Replace the internal-call trace with a session.tree snapshot. */
function setCodeCalls(calls: CodeCall[]): void {
  state.codeCalls = {}
  for (const c of calls) {
    const list = state.codeCalls[c.parent_call] ?? (state.codeCalls[c.parent_call] = [])
    list.push(c)
  }
  for (const list of Object.values(state.codeCalls)) list.sort((a, b) => a.seq - b.seq)
}

/** Replace nodes/branches/code calls with an authoritative `session.tree`. */
export function replaceTree(tree: SessionTreeResult): void {
  state.session = tree.session
  state.nodes = {}
  state.branches = {}
  for (const n of tree.nodes) state.nodes[n.id] = n
  for (const b of tree.branches) state.branches[b.id] = b
  setCodeCalls(tree.code_calls ?? [])
}

/** Re-fetch the open session's tree (authoritative after mutating ops). */
async function resyncTree(): Promise<void> {
  const id = state.sessionId
  if (!id) return
  const tree = await rumu.request<SessionTreeResult>('session.tree', { session: id })
  replaceTree(tree)
}

export async function loadPath(branch: BranchId): Promise<void> {
  try {
    const res = await rumu.request<{ nodes: Node[]; state: PathState }>('branch.path', {
      branch,
    })
    // Fold in any nodes we had not seen (authoritative from the server).
    for (const n of res.nodes) if (!state.nodes[n.id]) state.nodes[n.id] = n
    state.pathState = res.state
  } catch (err) {
    toast('error', t('errors.loadingBranch'), (err as Error).message)
  }
}

export async function selectBranch(id: BranchId): Promise<void> {
  state.activeBranch = id
  const b = state.branches[id]
  if (b) {
    // Optimistically follow the server's notion of active.
    if (state.session) state.session.active_branch = id
    // Keep the session list chip roughly in sync without a round trip.
    state.pathState = null
  }
  try {
    await rumu.request('branch.activate', { branch: id })
  } catch (err) {
    toast('error', t('errors.activate'), (err as Error).message)
  }
  await loadPath(id)
}

/**
 * Fork a new branch at `nodeId` and make it active. `label` names the action
 * in the error toast (fork / edit / regenerate).
 */
export async function forkAt(nodeId: NodeId, label = 'fork'): Promise<Branch | null> {
  try {
    const res = await rumu.request<{ branch: Branch }>('branch.fork', { node: nodeId })
    await resyncTree()
    state.activeBranch = res.branch.id
    if (state.session) state.session.active_branch = res.branch.id
    await loadPath(res.branch.id)
    return res.branch
  } catch (err) {
    toast(
      'error',
      t('errors.action', { label: t(`errors.labels.${label}` as MessageKey) }),
      (err as Error).message,
    )
    return null
  }
}

export async function forkFrom(nodeId: NodeId): Promise<void> {
  await forkAt(nodeId)
}

/**
 * Regenerate the reply to a prompt: the server forks at the user message,
 * replays its prompt (hooks and memory injection included) and streams a new
 * answer on that branch. The old reply stays on its own branch.
 */
export async function regenerateFrom(nodeId: NodeId): Promise<void> {
  try {
    const res = await rumu.request<{ branch: Branch }>('branch.regenerate', { node: nodeId })
    await resyncTree()
    state.activeBranch = res.branch.id
    if (state.session) state.session.active_branch = res.branch.id
    state.busy[res.branch.id] = true
    await loadPath(res.branch.id)
  } catch (err) {
    toast('error', t('errors.regenerate'), (err as Error).message)
  }
}

/**
 * The sub-agent branch spawned by the direct `agent` tool call `callId`
 * (recorded on the branch as `kind.call_id`). Prefers a child of the active
 * branch (the common case); falls back to any match (e.g. after a revert the
 * parent branch is no longer active).
 */
export function findAgentChild(callId: string): Branch | null {
  if (!callId) return null
  const matches = Object.values(state.branches).filter(
    (b): b is Branch => b.kind.type === 'agent' && !b.kind.via_plugin && b.kind.call_id === callId,
  )
  const isOwn = (b: Branch): boolean => b.kind.type === 'agent' && b.kind.parent_branch === state.activeBranch
  matches.sort((x, y) => Number(isOwn(y)) - Number(isOwn(x)))
  return matches[0] ?? null
}
