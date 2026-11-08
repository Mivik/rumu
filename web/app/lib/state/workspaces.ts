// The workspace registry (`workspace.*`) and the composer's workspaces: the
// open branch's (`branch.use_workspace` / `branch.set_workspaces`), or on the
// landing page the pick a new session starts with. Either way the pick is
// remembered for the next session (prefs). A registry change arrives as the
// `workspaces.updated` push, which re-reads the path.

import { computed } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../api'
import type { Workspace } from '../types'
import { prefs, type WorkspacePick } from './prefs'
import { state } from './state'
import { loadPath } from './tree'
import { toast } from './ui'

export async function refreshWorkspaces(): Promise<Workspace[]> {
  state.workspaces = await rumu.request<Workspace[]>('workspace.list')
  return state.workspaces
}

/** Add a workspace, or replace the one of the same name. */
export async function saveWorkspace(ws: Workspace): Promise<Workspace> {
  const res = await rumu.request<{ workspace: Workspace }>('workspace.upsert', {
    name: ws.name,
    root: ws.root,
    ssh: ws.ssh || undefined,
    read_only: ws.read_only,
  })
  await refreshWorkspaces()
  return res.workspace
}

export async function deleteWorkspace(name: string): Promise<void> {
  await rumu.request('workspace.delete', { name })
  await refreshWorkspaces()
}

/** What a new session of the assistant in view starts with: the last pick,
 *  else its deck's workspaces. Unregistered names drop out. */
function landingPick(): WorkspacePick {
  const deck = state.assistants.find((a) => a.id === state.assistant)
  const pick = prefs.workspaces[state.assistant] ?? { attached: deck?.workspaces ?? [], default: null }
  const attached = pick.attached.filter((n) => state.workspaces.some((w) => w.name === n))
  const fallback = attached[0] ?? '@rumu'
  const def =
    pick.default && (pick.default === '@rumu' || attached.includes(pick.default)) ? pick.default : fallback
  return { attached, default: def }
}

/** The composer's workspaces: `attached` names in order, the registered
 *  ones among them, and the default (`null` while the branch loads). */
export const composerWorkspaces = computed(() => {
  let pick: WorkspacePick
  if (state.activeBranch) {
    const ps = state.pathState
    if (!ps) return null
    pick = { attached: ps.attached, default: ps.default_workspace ?? null }
  } else {
    pick = landingPick()
  }
  const workspaces = pick.attached
    .map((n) => state.workspaces.find((w) => w.name === n))
    .filter((w): w is Workspace => !!w)
  return { ...pick, workspaces }
})

/** Make `name` the default workspace. */
export async function useWorkspace(name: string): Promise<void> {
  remember({ attached: composerWorkspaces.value?.attached ?? [], default: name })
  await onBranch('branch.use_workspace', { name })
}

/** The attached workspaces from here on, in order. */
export async function attachWorkspaces(names: string[]): Promise<void> {
  const def = composerWorkspaces.value?.default
  remember({ attached: names, default: def && names.includes(def) ? def : null })
  await onBranch('branch.set_workspaces', { names })
}

/** Keep the pick for the next session of the assistant it was made for. */
function remember(pick: WorkspacePick): void {
  const assistant = state.pathState?.assistant || state.assistant
  if (assistant) prefs.workspaces[assistant] = pick
}

/** Record a change on the open branch (none on the landing page). */
async function onBranch(method: string, params: Record<string, unknown>): Promise<void> {
  const branch = state.activeBranch
  if (!branch) return
  try {
    await rumu.request(method, { branch, ...params })
    await loadPath(branch)
  } catch (err) {
    toast('error', t('workspaces.branchError'), (err as Error).message)
  }
}
