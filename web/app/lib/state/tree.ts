// The open session's node tree and branches: loading, and moving the main
// line. Everything that replaces the tree goes through `replaceTree`,
// so the derived views always see a consistent snapshot.

import { rumu } from '../api'
import { t } from '../../i18n'
import type { Branch, BranchId, BranchPathResult, Node, NodeId, SessionId, SessionTreeResult } from '../types'
import { state } from './state'
import { syncLive } from './stream'
import { toast, withStop } from './ui'

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
export async function moveHead(node: NodeId, label: 'switch' | 'edit' = 'switch'): Promise<boolean> {
  const branch = state.session?.main_branch
  if (!branch) return false
  try {
    // The head moves by the `branch.updated` push, which comes first.
    const moved = await withStop((stop) => rumu.request('branch.move', { branch, node, stop }))
    if (moved === undefined) return false
    state.mainBranch = branch
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
