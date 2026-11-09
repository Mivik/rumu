// The workspace registry (`workspace.*`). Every registered workspace is
// addressable from every session; projects mount ordered subsets of them, so
// there is no per-session pick. A registry change arrives as the
// `workspaces.updated` push, which re-reads the registry and the projects.

import { rumu } from '../api'
import type { Workspace } from '../types'
import { state } from './state'

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
