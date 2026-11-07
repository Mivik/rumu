// The workspace registry (`workspace.*`) and the open branch's attached
// workspaces (`branch.use_workspace` / `branch.set_workspaces`). A registry
// change arrives as the `workspaces.updated` push, which re-reads the path.

import { t } from '../../i18n'
import { rumu } from '../api'
import type { Workspace } from '../types'
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

/** Make `name` the open branch's default workspace. */
export async function useWorkspace(name: string): Promise<void> {
  await onBranch('branch.use_workspace', { name })
}

/** The open branch's attached workspaces from here on, in order. */
export async function attachWorkspaces(names: string[]): Promise<void> {
  await onBranch('branch.set_workspaces', { names })
}

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
