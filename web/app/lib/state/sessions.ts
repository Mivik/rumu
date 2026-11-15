// Session lifecycle: listing, opening/closing, creating, prompting.

import { watch } from 'vue'
import { rumu } from '../api'
import { t } from '../../i18n'
import { commitRoute } from '../router'
import type { SessionView, StatsRange } from '../router'
import type {
  Branch,
  BranchId,
  Card,
  Assistant,
  DecisionAnswer,
  PendingDecision,
  Node,
  NodeId,
  Project,
  SessionId,
  SessionInfo,
  SessionTreeResult,
  TaskId,
  TaskInfo,
  Workspace,
} from '../types'
import type { ModelUse } from '../generated/ModelUse'
import { currentPick, models } from './models'
import { isUnseen } from './derived'
import { state } from './state'
import { findAgentChild, forkAt, loadPath, replaceTree, resyncTree } from './tree'
import { openAgentView } from './branchView'
import { requestComposerFocus, toast } from './ui'
import { chooseLandingProject, landingAssistant, landingProject, rememberSessionPicks } from './projects'

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
    const [sessions, assistants, decisions, workspaces, projects] = await Promise.all([
      rumu.request<SessionInfo[]>('session.list'),
      rumu.request<Assistant[]>('assistant.list'),
      rumu.request<PendingDecision[]>('decision.list'),
      rumu.request<Workspace[]>('workspace.list'),
      rumu.request<Project[]>('project.list'),
    ])
    state.workspaces = workspaces
    state.projects = projects
    state.sessions = sessions
    state.assistants = assistants
    if (!assistants.some((a) => a.id === state.assistant)) {
      state.assistant = defaultAssistant()
    }
    state.decisions = decisions
    await refreshCards()
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
 * pushes `/s/<id>` (leaving any page for it), boot and popstate leave it
 * alone, a failed boot replaces its stale URL with the landing page. */
export type OpenHistory = 'push' | 'replace' | 'none'

/** Latest open request; a slower earlier one is dropped, not applied. */
let openSeq = 0

export async function openSession(id: SessionId, opts: { history?: OpenHistory } = {}): Promise<boolean> {
  const seq = ++openSeq
  try {
    const tree = await rumu.request<SessionTreeResult>('session.tree', { session: id })
    if (seq !== openSeq) return false
    if (state.sessionId !== tree.session.id) {
      // Panels beside the chat belong to the session that opened them.
      state.artifact = null
      state.agentView = null
      state.taskDetail = null
    }
    state.sessionId = tree.session.id
    viewAssistant(tree.session.assistant)
    replaceTree(tree)
    state.activeBranch = tree.session.active_branch
    chooseLandingProject(tree.session.project ?? null)
    const history = opts.history ?? 'push'
    if (history !== 'none') {
      state.page = null
      commitRoute(history)
    }
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
 * history entry so Back returns to the session that was open. The composer is
 * where the user wants to be next, so it takes focus. */
export function goHome(): void {
  closeSession()
  state.page = null
  commitRoute('push')
  requestComposerFocus()
}

/** Show the open session another way; Back returns to the previous view. */
export function showView(view: SessionView): void {
  if (state.view === view && !state.statsRange) return
  state.view = view
  state.statsRange = null
  commitRoute('push')
}

/** The stats view, narrowed to `range` (the whole path when null). */
export function showStats(range: StatsRange | null): void {
  state.view = 'stats'
  state.statsRange = range
  commitRoute('push')
}

/** Back to the landing page (browser back past the first session). */
export function closeSession(): void {
  openSeq++ // drop any in-flight open
  state.booting = false // and any boot into a session URL still pending
  state.view = 'chat'
  state.statsRange = null
  state.sessionId = null
  state.session = null
  state.nodes = {}
  state.branches = {}
  state.activeBranch = null
  state.pathState = null
  state.deltas = {}
  state.busy = {}
  state.queuedPrompts = {}
  state.retrying = {}
  state.toolProgress = {}
  state.tasks = {}
  state.taskOutput = {}
  state.taskDetail = null
  state.agentView = null
  state.artifact = null
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

/** The branch a background sub-agent task runs on, once it has one. */
export function taskChild(id: TaskId): BranchId | null {
  const tk = state.tasks[id]
  if (tk?.kind.type !== 'agent') return null
  return tk.kind.child_branch ?? findAgentChild(tk.call_id)?.id ?? null
}

/** Toggle a task beside the chat (it replaces an open artifact or sub-agent):
 *  a sub-agent's transcript, or another task's output. */
export function openTaskDetail(id: TaskId): void {
  if (state.tasks[id]?.kind.type === 'agent') {
    const child = taskChild(id)
    if (child) openAgentView(state.agentView === child ? null : child)
    return
  }
  state.taskDetail = state.taskDetail === id ? null : id
  if (!state.taskDetail) return
  state.artifact = null
  state.agentView = null
}

/** Start a session of the landing picks: who it runs as and the project it
 *  works in (`null`: none). */
export async function newSession(): Promise<boolean> {
  try {
    const assistant = landingAssistant.value || undefined
    const project = landingProject.value
    const res = await rumu.request<{ session: SessionInfo; branch: Branch }>('session.create', {
      assistant,
      project,
    })
    rememberSessionPicks(project, assistant ?? '')
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

/** From the landing page: start the next session in `project`. */
export function startSessionIn(project: string | null): void {
  chooseLandingProject(project)
  goHome()
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
  commitRoute('replace')
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

/** Have the title model write the session's title anew; it arrives as a
 *  `session.updated` push. */
export async function retitleSession(id: SessionId): Promise<void> {
  try {
    await rumu.request('session.retitle', { session: id })
  } catch (err) {
    toast('error', t('errors.retitleSession'), (err as Error).message)
  }
}

/** An attachment sent with a prompt: the uploaded blob, plus the name it was
 * attached under. */
export interface PromptAttachment {
  hash: string
  name?: string
}

/** What the composer shows, carried by a prompt or regeneration: the
 *  server records it when it differs from the branch it lands on. */
export interface Choice {
  /** Null is the assistant's model. */
  model?: ModelUse | null
}

export function composerChoice(): Choice {
  return { model: currentPick() }
}

/** The session's unsent picks are on the branch now. */
function settlePicks(): void {
  if (state.sessionId) delete models.pending[state.sessionId]
}

/**
 * Send a message on the active branch. With no session open (the landing
 * state), start a new session first and send the text as its first prompt.
 * `attachments` name already-uploaded images the text places with
 * `[Image #N]` anchors (empty text is allowed when there is one). `choice`
 * is the composer's pick, taken before any fork. Returns whether the prompt
 * was accepted.
 */
export async function sendPrompt(
  text: string,
  attachments: PromptAttachment[] = [],
  choice: Choice = composerChoice(),
): Promise<boolean> {
  if (!text.trim() && !attachments.length) return false
  if (!state.sessionId) {
    if (!(await newSession())) return false
    // The new session's root holds who and where already.
    choice = { model: choice.model }
  }
  const branch = state.activeBranch
  if (!branch) return false
  state.busy[branch] = true
  try {
    const params: Record<string, unknown> = { branch, text, ...choice }
    if (attachments.length) params.attachments = attachments
    await rumu.request('prompt', params)
    settlePicks()
    return true
  } catch (err) {
    state.busy[branch] = false
    toast('error', t('errors.prompt'), (err as Error).message)
    return false
  }
}

/**
 * Regenerate the reply to a prompt: the server forks at the user message,
 * records the composer's model where it differs, replays the prompt (hooks
 * and memory injection included) and streams a new answer on that branch,
 * replacing the branch it leaves. The old reply stays reachable from the
 * message's variant switcher.
 */
export async function regenerateFrom(nodeId: NodeId): Promise<void> {
  try {
    const res = await rumu.request<{ branch: Branch }>('branch.regenerate', {
      node: nodeId,
      ...composerChoice(),
    })
    settlePicks()
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
 * `branch.retry`: run the turn again on the active branch, in place — no
 * prompt event, no new user node; the failed marker stays on the path. The
 * composer's pick goes along, so a failure fixed by switching the model
 * retries on the new one. The server refuses branches that already have
 * their reply.
 */
export async function retryBranch(): Promise<boolean> {
  const branch = state.activeBranch
  if (!branch) return false
  state.busy[branch] = true
  try {
    await rumu.request('branch.retry', { branch, ...composerChoice() })
    settlePicks()
    return true
  } catch (err) {
    state.busy[branch] = false
    toast('error', t('errors.retry'), (err as Error).message)
    return false
  }
}

/** `branch.compact`: summarize the active branch's older context now, the
 *  summary focusing on `focus` when given. */
export async function compactBranch(focus: string): Promise<boolean> {
  const branch = state.activeBranch
  if (!branch) return false
  state.busy[branch] = true
  try {
    await rumu.request('branch.compact', focus ? { branch, focus } : { branch })
    return true
  } catch (err) {
    state.busy[branch] = false
    toast('error', t('errors.compact'), (err as Error).message)
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
 * alternative stays reachable from the user message's variant switcher. It
 * runs on the composer's pick, not the one the original was sent with. An
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
  const choice = composerChoice()
  if (!(await forkAt(before, 'edit'))) return false
  return sendPrompt(next, attachments, choice)
}
