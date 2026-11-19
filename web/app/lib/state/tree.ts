// The open session's node tree and branches: loading, switching and
// forking. Everything that replaces the tree goes through `replaceTree`,
// so the derived views always see a consistent snapshot.

import { rumu } from '../api'
import { t } from '../../i18n'
import type { MessageKey } from '../../i18n'
import type { Branch, BranchId, BranchPathResult, Node, NodeId, SessionId, SessionTreeResult } from '../types'
import { state } from './state'
import { syncLive } from './stream'
import { toast } from './ui'

/** Replace nodes/branches with an authoritative `session.tree`. */
export function replaceTree(tree: SessionTreeResult): void {
  state.session = tree.session
  state.nodes = {}
  state.branches = {}
  state.agentNodes = {}
  state.usage = tree.usage
  for (const n of tree.nodes) state.nodes[n.id] = n
  for (const b of tree.branches) state.branches[b.id] = b
  syncLive(tree)
}

/** The open session's tree, sub-agents' nodes left out (`ensureAgentNodes`
 *  fetches one's when it is opened). */
export function fetchTree(session: SessionId): Promise<SessionTreeResult> {
  return rumu.request<SessionTreeResult>('session.tree', { session, agents: false })
}

/** Re-fetch the open session's tree (authoritative after mutating ops). */
export async function resyncTree(): Promise<void> {
  const id = state.sessionId
  if (!id) return
  replaceTree(await fetchTree(id))
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

/** Switch to `branch`: the message variant switcher's pick. */
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
 * Fork a new branch at `nodeId` and make it active — how an edited prompt
 * steps away from the turn it replaces. `label` names the action in the
 * error toast (edit); the branch left behind stays reachable from the
 * message's variant switcher.
 */
export async function forkAt(nodeId: NodeId, label = 'edit'): Promise<Branch | null> {
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

/**
 * The sub-agent branch spawned by the direct `agent` tool call `callId`
 * (recorded on the branch as `kind.call_id`). Prefers a child of the active
 * branch (the common case); falls back to any match (e.g. on a variant the
 * call's parent branch is no longer active).
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
