// Session lifecycle: listing, opening/closing, creating, prompting.

import { rumu } from '../api'
import { t } from '../../i18n'
import { routeSession } from '../router'
import type { Card, Node, SessionId, SessionInfo, SessionTreeResult, TaskId, TaskInfo } from '../types'
import { state } from './state'
import { forkAt, loadPath, replaceTree } from './tree'
import { toast } from './ui'

export async function refreshLists(): Promise<void> {
  try {
    const [sessions, cards] = await Promise.all([
      rumu.request<SessionInfo[]>('session.list'),
      rumu.request<Card[]>('card.list'),
    ])
    state.sessions = sessions
    state.cards = cards
  } catch (err) {
    toast('error', t('errors.listingSessions'), (err as Error).message)
  }
}

/** Open a session. `history` decides what happens to the address bar: a pick
 * pushes `/s/<id>`, boot and popstate leave it alone, a failed boot replaces
 * its stale URL with the landing page. */
export type OpenHistory = 'push' | 'replace' | 'none'

/** Latest open request; a slower earlier one is dropped, not applied. */
let openSeq = 0

export async function openSession(id: SessionId, opts: { history?: OpenHistory } = {}): Promise<boolean> {
  const seq = ++openSeq
  try {
    const tree = await rumu.request<SessionTreeResult>('session.tree', { session: id })
    if (seq !== openSeq) return false
    state.sessionId = tree.session.id
    replaceTree(tree)
    // A newly loaded history has no live failures; anything still listed here
    // belonged to the view we just left.
    state.failedTurns = {}
    state.activeBranch = tree.session.active_branch
    const history = opts.history ?? 'push'
    if (history !== 'none') routeSession(state.sessionId, history)
    await loadPath(tree.session.active_branch)
    void refreshTasks(tree.session.id)
    refreshLists() // background; updated_at may have changed
    return true
  } catch (err) {
    toast('error', t('errors.loadingSession'), (err as Error).message)
    return false
  }
}

/** Back to the landing page by intent (the wordmark was clicked), keeping the
 * history entry so Back returns to the session that was open. */
export function goHome(): void {
  closeSession()
  routeSession(null, 'push')
}

/** Back to the landing page (browser back past the first session). */
export function closeSession(): void {
  openSeq++ // drop any in-flight open
  state.booting = false // and any boot into a session URL still pending
  state.sessionId = null
  state.session = null
  state.nodes = {}
  state.branches = {}
  state.codeCalls = {}
  state.activeBranch = null
  state.pathState = null
  state.deltas = {}
  state.busy = {}
  state.queuedPrompts = {}
  state.retrying = {}
  state.failedTurns = {}
  state.toolProgress = {}
  state.tasks = {}
  state.taskOutput = {}
  state.taskDetail = null
}

/** Resync the session's runtime task table (`task.list`). */
export async function refreshTasks(session: SessionId): Promise<void> {
  try {
    const list = await rumu.request<TaskInfo[]>('task.list', { session })
    if (state.sessionId !== session) return
    const next: Record<string, TaskInfo> = {}
    for (const t of list) next[t.id] = t
    state.tasks = next
  } catch (err) {
    toast('error', t('errors.listingTasks'), (err as Error).message)
  }
}

/** `task.kill`: cancel a task (running or already finished) and record the
 * final status. */
export async function killTask(id: TaskId): Promise<void> {
  try {
    const res = await rumu.request<{ task: TaskInfo }>('task.kill', { id })
    state.tasks[res.task.id] = res.task
  } catch (err) {
    toast('error', t('errors.killing', { id }), (err as Error).message)
  }
}

/** Toggle the task detail (buffered output) in the tasks panel. */
export function openTaskDetail(id: TaskId): void {
  state.taskDetail = state.taskDetail === id ? null : id
}

export async function newSession(): Promise<boolean> {
  try {
    const res = await rumu.request<{ session: SessionInfo }>('session.create', {})
    // The server may have pushed session.updated for it already.
    const i = state.sessions.findIndex((s) => s.id === res.session.id)
    if (i >= 0) state.sessions[i] = res.session
    else state.sessions.push(res.session)
    return await openSession(res.session.id)
  } catch (err) {
    toast('error', t('errors.newSession'), (err as Error).message)
    return false
  }
}

/** An attachment sent with a prompt: the uploaded blob, plus the name it was
 * attached under. */
export interface PromptAttachment {
  hash: string
  name?: string
}

/**
 * Send a message on the active branch. With no session open (the landing
 * state), start a new session first and send the text as its first prompt.
 * `attachments` name already-uploaded images the text places with
 * `[Image #N]` anchors (empty text is allowed when there is one). Returns
 * whether the prompt was accepted.
 */
export async function sendPrompt(text: string, attachments: PromptAttachment[] = []): Promise<boolean> {
  if (!text.trim() && !attachments.length) return false
  if (!state.sessionId && !(await newSession())) return false
  const branch = state.activeBranch
  if (!branch) return false
  state.busy[branch] = true
  try {
    await rumu.request('prompt', attachments.length ? { branch, text, attachments } : { branch, text })
    return true
  } catch (err) {
    state.busy[branch] = false
    toast('error', t('errors.prompt'), (err as Error).message)
    return false
  }
}

export async function stopTurn(): Promise<void> {
  const branch = state.activeBranch
  if (!branch) return
  try {
    await rumu.request('abort', { branch })
  } catch (err) {
    toast('error', t('errors.abort'), (err as Error).message)
  }
}

/**
 * Edit a sent prompt: fork from just before it and send the new text there.
 * The branch holding the original message and reply is abandoned; its
 * alternative stays reachable from the user message's switcher. An edited
 * message resends its attachments unchanged — the edited text's anchors say
 * where they sit.
 */
export async function editPrompt(
  node: Node,
  text: string,
  attachments: PromptAttachment[] = [],
): Promise<boolean> {
  const next = text.trim()
  if ((!next && !attachments.length) || !state.sessionId) return false
  const before = node.parent
  if (!before) {
    toast('error', t('errors.editFirst'))
    return false
  }
  if (!(await forkAt(before, 'edit', true))) return false
  return sendPrompt(next, attachments)
}
