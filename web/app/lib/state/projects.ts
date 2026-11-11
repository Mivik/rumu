// Project decks and the landing picks a new session starts with. A project is
// a deck (`project: true`) plus an ordered list of mounted workspaces (the
// first is its default). Sessions run as one assistant in at most one
// project; moving one is a branch marker (`session.move`).

import { computed } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../api'
import { DECK_NAME_MAX } from '../deckName'
import type { DeckIcon, Project, SessionId } from '../types'
import { isUnseen } from './derived'
import { prefs } from './prefs'
import { state } from './state'
import { toast } from './ui'

/** Every project deck, sorted by name. */
export async function refreshProjects(): Promise<Project[]> {
  state.projects = await rumu.request<Project[]>('project.list')
  return state.projects
}

export async function createProject(
  name: string,
  workspaces: string[] = [],
  icon?: DeckIcon,
  privateDeck = false,
): Promise<Project> {
  const res = await rumu.request<{ project: Project }>('project.create', {
    name,
    workspaces,
    icon,
    private: privateDeck,
  })
  await refreshProjects()
  return res.project
}

/** Rename a project, set its seal (an empty one clears it) and/or replace
 *  its ordered workspaces. */
export async function updateProject(
  id: string,
  changes: { name?: string; icon?: DeckIcon; workspaces?: string[]; private?: boolean },
): Promise<Project> {
  const res = await rumu.request<{ project: Project }>('project.update', { id, ...changes })
  await refreshProjects()
  return res.project
}

/** Delete a project deck; its workspace list goes with it. */
export async function deleteProject(id: string): Promise<void> {
  await rumu.request('library.deck_delete', { id })
  await refreshProjects()
}

/** Move a session into a project, or out of its project (`null`). The
 *  session's new project also arrives as a `session.updated` push. */
export async function moveSession(session: SessionId, project: string | null): Promise<void> {
  const entry = state.sessions.find((s) => s.id === session)
  const before = entry?.project
  // The move's own markers are nothing new to the user who made it.
  const seen = !!entry && !isUnseen(entry)
  setSessionProject(session, project)
  try {
    await rumu.request('session.move', { session, project })
    if (seen) await rumu.request('session.seen', { session })
  } catch (err) {
    setSessionProject(session, before ?? null)
    toast('error', t('errors.moveSession'), (err as Error).message)
  }
}

function setSessionProject(session: SessionId, project: string | null): void {
  const entry = state.sessions.find((s) => s.id === session)
  if (!entry) return
  if (project) entry.project = project
  else delete entry.project
}

// ---- landing picks ----

function defaultAssistantId(): string {
  return state.assistants.find((a) => a.default)?.id ?? ''
}

/** The project a new session starts in (`null`: none): the last one in view. */
export const landingProject = computed<string | null>(() => {
  const wanted = prefs.lastProject
  return wanted && state.projects.some((p) => p.id === wanted) ? wanted : null
})

/** The assistant a new session runs as: the last one used in that project,
 *  else the default assistant. */
export const landingAssistant = computed<string>(() => {
  const wanted = prefs.assistantByProject[landingProject.value ?? ''] ?? defaultAssistantId()
  return state.assistants.some((a) => a.id === wanted) ? wanted : defaultAssistantId()
})

/** The project a new session starts in from now on: picked on the landing
 *  page, or the one of the session opened last. */
export function chooseLandingProject(project: string | null): void {
  prefs.lastProject = project
}

export function chooseLandingAssistant(assistant: string): void {
  prefs.assistantByProject[landingProject.value ?? ''] = assistant
}

/** A session was created with these picks: the next one starts the same way. */
export function rememberSessionPicks(project: string | null, assistant: string): void {
  prefs.lastProject = project
  if (assistant) prefs.assistantByProject[project ?? ''] = assistant
}

// ---- names from folders ----

const WORKSPACE_NAME = /^[a-z][a-z0-9.-]+$/

function folderBase(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? ''
}

/** `name`, or `name-2`, `name-3`, … when it is taken. */
function unique(name: string, taken: (n: string) => boolean): string {
  let out = name
  for (let i = 2; taken(out); i++) out = `${name}-${i}`
  return out
}

/** The workspace name a folder suggests: lowercased, every invalid character
 *  folded to `-` (an unusable name becomes `ws`), not in `taken`. */
export function folderWorkspaceName(path: string, taken: string[]): string {
  const name = folderBase(path)
    .toLowerCase()
    .replace(/[^a-z0-9.-]+/g, '-')
  return unique(WORKSPACE_NAME.test(name) ? name : 'ws', (n) => taken.includes(n))
}

/** The project name a folder suggests: its base name, not taken by a known
 *  project or assistant. */
export function folderProjectName(path: string): string {
  const base = folderBase(path)
    .replace(/[\u0000-\u001f\u007f]+/g, ' ')
    .trim()
  const name = Array.from(base).slice(0, DECK_NAME_MAX).join('') || 'project'
  const taken = (n: string) =>
    state.projects.some((p) => p.name.toLowerCase() === n.toLowerCase()) ||
    state.assistants.some((a) => a.name.toLowerCase() === n.toLowerCase())
  return unique(name, taken)
}
