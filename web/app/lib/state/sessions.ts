// Session lifecycle: listing, opening/closing, creating, prompting.

import { watch } from 'vue'
import { rumu } from '../api'
import { t } from '../../i18n'
import { routeSession } from '../router'
import type {
  Card,
  Assistant,
  DecisionAnswer,
  PendingDecision,
  Node,
  NodeId,
  SessionId,
  SessionInfo,
  SessionTreeResult,
  TaskId,
  TaskInfo,
  Workspace,
} from '../types'
import { models, takePending } from './models'
import { isUnseen } from './derived'
import { state } from './state'
import { forkAt, loadPath, replaceTree } from './tree'
import { toast } from './ui'

/** `card.list`: what the open branch has mounted (`card_mount` included), else
 * the default mounts of the assistant in view. An assistant gone from the
 * config has no shelf; its old sessions still open, with no cards. */
async function listCards(): Promise<Card[]> {
  if (state.activeBranch) return rumu.request<Card[]>('card.list', { branch: state.activeBranch })
  if (!state.assistants.some((c) => c.id === state.assistant)) return []
  return rumu.request<Card[]>('card.list', { assistant: state.assistant })
}

/** The id of the default assistant ('' before the roster has loaded). */
export function defaultAssistant(): string {
  return state.assistants.find((a) => a.default)?.id ?? ''
}

export async function refreshLists(): Promise<void> {
  try {
    const [sessions, assistants, decisions, workspaces] = await Promise.all([
      rumu.request<SessionInfo[]>('session.list'),
      rumu.request<Assistant[]>('assistant.list'),
      rumu.request<PendingDecision[]>('decision.list'),
      rumu.request<Workspace[]>('workspace.list'),
    ])
    state.workspaces = workspaces
    state.sessions = sessions
    state.assistants = assistants
    if (!assistants.some((a) => a.id === state.assistant)) {
      state.assistant = defaultAssistant()
    }
    state.decisions = decisions
    state.cards = await listCards()
  } catch (err) {
    toast('error', t('errors.listingSessions'), (err as Error).message)
  }
}

/** Re-read the configured assistants (`assistant.list`), e.g. after editing them in the
 *  assistant config dialog. */
export async function refreshAssistants(): Promise<Assistant[]> {
  state.assistants = await rumu.request<Assistant[]>('assistant.list')
  return state.assistants
}

// The shelf follows the open branch's mount table.
watch(
  () => state.activeBranch,
  () => void refreshCards(),
)

/** In-flight `card.list` refresh, so a burst of writes costs one request. */
let cardsRefresh: Promise<void> | null = null
let cardsDirty = false

/** A card was written or changed behind the live card list: re-read it, so
 * the drawer's "latest version" is actually the latest. A write landing
 * while a refresh is in flight marks it dirty and runs one more pass. */
export function refreshCards(): Promise<void> {
  if (cardsRefresh) {
    cardsDirty = true
    return cardsRefresh
  }
  cardsRefresh = (async () => {
    try {
      do {
        cardsDirty = false
        state.cards = await listCards()
      } while (cardsDirty)
    } catch (err) {
      toast('error', t('errors.listingCards'), (err as Error).message)
    } finally {
      cardsRefresh = null
    }
  })()
  return cardsRefresh
}

/** Answer the oldest pending question. An "always" grant changes what the
 * assistant roster reports, so the assistants are re-read. */
export async function answerDecision(id: string, answer: DecisionAnswer): Promise<void> {
  state.decisions = state.decisions.filter((d) => d.id !== id)
  try {
    await rumu.request('decision.answer', { id, answer })
    if ('scope' in answer && answer.scope === 'always')
      state.assistants = await rumu.request<Assistant[]>('assistant.list')
  } catch (err) {
    toast('error', t('errors.answer'), (err as Error).message)
  }
}

/** Bring `assistant` into view: its sessions, its library, and new sessions run as
 * it. A session of another assistant stays open until the user leaves it. */
export function viewAssistant(assistant: string): void {
  if (state.assistant === assistant) return
  state.assistant = assistant
  void refreshCards()
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
    viewAssistant(tree.session.assistant)
    replaceTree(tree)
    // A newly loaded history has no live failures; anything still listed here
    // belonged to the view we just left.
    state.failedTurns = {}
    state.activeBranch = tree.session.active_branch
    const history = opts.history ?? 'push'
    if (history !== 'none') routeSession(state.sessionId, history)
    await loadPath(tree.session.active_branch)
    markSeen()
    void refreshTasks(tree.session.id)
    refreshLists() // background; updated_at may have changed
    return true
  } catch (err) {
    toast('error', t('errors.loadingSession'), (err as Error).message)
    return false
  }
}

/** The open session is on screen: record that its updates are seen (a session
 *  never seen before starts being tracked). The server pushes the result. */
export function markSeen(): void {
  const s = state.session
  if (!s || document.visibilityState !== 'visible') return
  if (s.seen_at != null && !isUnseen(s)) return
  rumu.request('session.seen', { session: s.id }).catch(() => {})
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
    const res = await rumu.request<{ session: SessionInfo }>('session.create', {
      assistant: state.assistant || undefined,
    })
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

/**
 * Fork the conversation up to `node` into a new session and open it. The
 * source session is untouched; sub-agent branches stay behind (their call
 * rows keep the results they produced).
 */
export async function forkSession(node: NodeId): Promise<void> {
  try {
    const res = await rumu.request<{ session: SessionInfo }>('session.fork', { node })
    await openSession(res.session.id)
  } catch (err) {
    toast('error', t('errors.forkSession'), (err as Error).message)
  }
}

/** A session is gone (deleted here or in another tab): drop it from the list
 * and, when it was the open one, land on the home page. The address entry is
 * replaced so Back cannot reopen a dead session. */
export function forgetSession(id: SessionId): void {
  state.sessions = state.sessions.filter((s) => s.id !== id)
  if (state.sessionId !== id) return
  closeSession()
  routeSession(null, 'replace')
}

/** Delete a session and its stored history for good. */
export async function deleteSession(id: SessionId): Promise<void> {
  try {
    await rumu.request('session.delete', { session: id })
    forgetSession(id)
  } catch (err) {
    toast('error', t('errors.deleteSession'), (err as Error).message)
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
    const model = takePending(branch)
    const params: Record<string, unknown> = { branch, text }
    if (attachments.length) params.attachments = attachments
    if (model) params.model = model
    try {
      await rumu.request('prompt', params)
    } catch (err) {
      if (model) models.pending[branch] = model
      throw err
    }
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
 * Edit a sent prompt: fork from just before it and send the new text there,
 * replacing the branch that held the original message and reply — its
 * alternative stays reachable from the user message's variant switcher. An
 * edited message resends its attachments unchanged — the edited text's
 * anchors say where they sit.
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
  if (!(await forkAt(before, 'edit'))) return false
  return sendPrompt(next, attachments)
}
