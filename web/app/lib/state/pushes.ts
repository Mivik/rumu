// WebSocket push handling and boot. This is the only module that translates
// server pushes into state mutations; it calls actions from the sibling
// modules rather than mutating anything itself (beyond its own slices).

import { rumu } from '../api'
import {
  commitRoute,
  pageFromLocation,
  rangeFromLocation,
  sessionIdFromLocation,
  viewFromLocation,
} from '../router'
import type {
  BranchUpdatedPush,
  PendingDecision,
  DecisionResolvedPush,
  HookFiredPush,
  LlmDeltaPush,
  Node,
  NodeAppendedPush,
  PromptBlockedPush,
  PromptQueuedPush,
  SessionDeletedPush,
  SessionId,
  SessionUpdatedPush,
  TaskInfoPush,
  TaskProgressPush,
  ToolProgressPush,
  TurnErrorPush,
  TurnPush,
  TurnRetryingPush,
  WarningPush,
  Workspace,
} from '../types'
import { MAX_TRACE, state } from './state'
import { applyDelta } from './stream'
import { artifactOf } from '../artifacts'
import {
  closeSession,
  forgetSession,
  markSeen,
  openSession,
  refreshAssistants,
  refreshCards,
  refreshLists,
} from './sessions'
import { onSettingsUpdated, refreshSettings } from './models'
import type { SettingsInfo } from '../generated/SettingsInfo'
import { refreshLibrary } from './library'
import { refreshProjects } from './projects'
import { loadPath } from './tree'
import { toast } from './ui'
import { t } from '../../i18n'

let traceSeq = 1

/** Live tool/task output kept in memory (chars). */
const MAX_LIVE_OUTPUT = 1_000_000

/** Did this node change the card library on disk? A write/edit records a
 * `card_use`; the memory hook records `card_update` notices for cards that
 * changed elsewhere. Both make open card windows and deck counts stale. */
function cardChanged(node: Node): boolean {
  if (node.body.type !== 'note') return false
  const data = node.body.content.data
  if (!data) return false
  if (data.tag === 'memory.card_update') return true
  // The mount table changed: the shelf follows the branch.
  if (data.tag === 'card.mount' || data.tag === 'card.unmount') return true
  if (data.tag !== 'card_use') return false
  const op = (data.data as { op?: unknown }).op
  return op === 'write' || op === 'edit'
}

/** Did this node change the branch's state (workspaces, model or card
 *  mounts)? */
function stateChanged(node: Node): boolean {
  const tag = node.body.type === 'note' ? node.body.content.data?.tag : undefined
  return (
    tag === 'workspace.use' ||
    tag === 'workspace.set' ||
    tag === 'model.use' ||
    tag === 'card.mount' ||
    tag === 'card.unmount'
  )
}

/** `workspaces.updated`: the registry or a project's workspace list changed. */
function onWorkspacesUpdated(p: { workspaces: Workspace[]; projects: Record<string, string[]> }): void {
  state.workspaces = p.workspaces
  void refreshProjects().catch(() => {})
  if (state.activeBranch) void loadPath(state.activeBranch)
}

/** A deck's configuration or the roster changed (here, in another tab, or
 *  by a model writing `@rumu:/decks`): re-read what shows it. */
function onDecksUpdated(): void {
  void refreshAssistants().catch(() => {})
  void refreshProjects().catch(() => {})
  if (state.page?.page === 'decks') void refreshLibrary()
}

function onNodeAppended(p: NodeAppendedPush): void {
  const { branch, node } = p
  if (branch === state.activeBranch && stateChanged(node)) void loadPath(branch)
  if (cardChanged(node)) {
    void refreshCards()
    // The model may be writing cards while the user browses the library.
    if (state.page?.page === 'decks') void refreshLibrary()
  }
  if (state.sessionId && node.session === state.sessionId) {
    if (node.body.type === 'assistant' && !state.nodes[node.id]) {
      const u = node.body.usage
      state.usage.calls++
      state.usage.input += u.prompt_tokens
      state.usage.cached += u.cached_tokens
      state.usage.output += u.completion_tokens
    }
    state.nodes[node.id] = node
    // A node extending a branch's head grows its path live (the active
    // one, or a sub-agent open beside it).
    const b = state.branches[branch]
    if (b && node.parent === b.head) {
      b.head = node.id
    }
  }
  // A file the model just showed opens beside the chat on a wide screen, or
  // replaces the one already open.
  if (
    branch === state.activeBranch &&
    node.body.type === 'tool_result' &&
    !node.body.is_error &&
    artifactOf(node.body.content) &&
    (state.artifact || (!state.taskDetail && window.matchMedia('(min-width: 80rem)').matches))
  ) {
    state.artifact = node.id
    state.taskDetail = null
  }
  // The finalized Assistant node replaces the streaming buffer.
  if (node.body.type === 'assistant') delete state.deltas[branch]
  // A finished tool result replaces its live output.
  if (node.body.type === 'tool_result') {
    delete state.toolProgress[`${branch}:${node.body.call_id}`]
  }
  // A queued steering prompt is no longer pending once its user node lands.
  if (node.body.type === 'user') {
    const queued = state.queuedPrompts[branch]
    if (queued?.length) {
      const i = queued.findIndex((q) => q.id === node.id)
      if (i >= 0) queued.splice(i, 1)
      if (!queued.length) delete state.queuedPrompts[branch]
    }
  }
}

/** A prompt accepted while the branch was busy; shown pending until written. */
function onPromptQueued(p: PromptQueuedPush): void {
  const list = state.queuedPrompts[p.branch] ?? (state.queuedPrompts[p.branch] = [])
  list.push({ id: p.node, text: p.text })
}

function onLlmDelta(p: LlmDeltaPush): void {
  // The attempt is live again; its retry notice stops applying.
  delete state.retrying[p.branch]
  applyDelta(p)
}

/** A foreground tool's live output (the run's running rows). */
function onToolProgress(p: ToolProgressPush): void {
  const key = `${p.branch}:${p.call_id}`
  let buf = state.toolProgress[key]
  if (!buf) {
    buf = { branch: p.branch, call_id: p.call_id, text: '', data: null, at: 0 }
    state.toolProgress[key] = buf
  }
  buf.at = Date.now()
  if (p.append) buf.text = (buf.text + p.append).slice(-MAX_LIVE_OUTPUT)
  if (p.data) buf.data = p.data
}

/** `task.started` / `task.done`: the task table's authoritative snapshots. */
function onTaskInfo(p: TaskInfoPush): void {
  state.tasks[p.task.id] = p.task
  // A finished task keeps its buffered output for the detail view; a running
  // one starts empty unless progress already arrived.
  state.taskOutput[p.task.id] ??= ''
}

function onTaskProgress(p: TaskProgressPush): void {
  if (p.kind && state.tasks[p.id]) state.tasks[p.id]!.kind = p.kind
  const prev = state.taskOutput[p.id] ?? ''
  if (p.append) state.taskOutput[p.id] = (prev + p.append).slice(-MAX_LIVE_OUTPUT)
  if (p.data && state.tasks[p.id]) state.tasks[p.id]!.data = p.data
}

function onBranchUpdated(p: BranchUpdatedPush): void {
  if (state.sessionId && p.branch.session === state.sessionId) {
    const old = state.branches[p.branch.id]
    state.branches[p.branch.id] = p.branch
    // A head moved rather than grown (a variant picked, an edit — maybe in
    // another tab): the shown path's state is another one.
    const moved = old && old.head !== p.branch.head && state.nodes[p.branch.head]?.parent !== old.head
    if (moved && p.branch.id === state.activeBranch) void loadPath(p.branch.id)
  }
}

function onSessionUpdated(p: SessionUpdatedPush): void {
  const s = p.session
  const i = state.sessions.findIndex((x) => x.id === s.id)
  if (i >= 0) state.sessions[i] = s
  else state.sessions.push(s)
  // The model can create a project; learn it when a session names one.
  if (s.project && !state.projects.some((x) => x.id === s.project)) void refreshProjects().catch(() => {})
  if (state.sessionId === s.id) {
    state.session = s
    markSeen()
  }
}

/** Another tab deleted a session; this one drops it (and leaves it if open). */
function onSessionDeleted(p: SessionDeletedPush): void {
  forgetSession(p.session.id)
}

function onTurnStarted(p: TurnPush): void {
  state.busy[p.branch] = true
  delete state.retrying[p.branch]
}

function onTurnDone(p: TurnPush): void {
  state.busy[p.branch] = false
  delete state.deltas[p.branch]
  delete state.retrying[p.branch]
  delete state.compacting[p.branch]
}

function onTurnError(p: TurnErrorPush): void {
  state.busy[p.branch] = false
  delete state.deltas[p.branch]
  delete state.retrying[p.branch]
  delete state.compacting[p.branch]
  // A hook refused the prompt before the turn started: nothing was recorded
  // and no reply is owed — a toast, never a failure row.
  if (p.blocked) {
    toast('error', p.message)
    return
  }
  // A recorded failure shows as the error row where the missing reply would
  // be (`owedReply`, from its marker on the path). A branch that is not on
  // screen has no row to land in, and an unrecorded failure no marker, so
  // those still need a toast.
  if (!p.recorded || p.branch !== state.activeBranch) toast('error', p.message)
}

/** `prompt.blocked`: a steering prompt was refused by a hook while the turn
 * kept running. The turn is untouched, so this must not touch busy or
 * deltas — the refusal is a toast only. */
function onPromptBlocked(p: PromptBlockedPush): void {
  toast('error', p.message)
}

/** `turn.retrying`: the model call failed transiently and will be replayed
 * after a backoff; shown where the thinking indicator is. */
function onTurnRetrying(p: TurnRetryingPush): void {
  state.retrying[p.branch] = {
    attempt: p.attempt,
    max: p.max,
    until: Date.now() + p.retry_in_ms,
    message: p.message,
  }
}

/** Server-side non-fatal warning: show it, never interrupt the turn. */
function onWarning(p: WarningPush): void {
  toast('error', p.message)
}

function onHookFired(p: HookFiredPush): void {
  // Attribute to the open session when the branch belongs to it (or is unknown
  // while one of its branches is busy — mid-turn sub-agents etc.).
  let session: SessionId | null = null
  const known = state.branches[p.branch]
  if (known && known.session === state.sessionId) session = state.sessionId
  else if (!known && state.sessionId && Object.values(state.busy).some(Boolean)) {
    session = state.sessionId
  }
  state.hookTrace.push({
    seq: traceSeq++,
    at: Date.now(),
    session,
    branch: p.branch,
    event: p.event,
    plugin: p.plugin,
    hook_id: p.hook_id,
    effects: p.effects ?? [],
    decision: p.decision ?? { type: 'pass' },
    parent_call: p.parent_call,
  })
  if (state.hookTrace.length > MAX_TRACE) state.hookTrace.splice(0, state.hookTrace.length - MAX_TRACE)
}

let started = false

export function startApp(): void {
  if (started) return
  started = true

  // A refresh on `/s/<id>` names its session in the address. From the first
  // paint, hold the chat pane for it instead of flashing the landing page
  // while the boot requests are in flight.
  state.booting = sessionIdFromLocation() !== null
  state.view = viewFromLocation()
  state.statsRange = rangeFromLocation()
  state.page = pageFromLocation()

  rumu.onPush('node.appended', (p) => onNodeAppended(p as unknown as NodeAppendedPush))
  rumu.onPush('prompt.queued', (p) => onPromptQueued(p as unknown as PromptQueuedPush))
  rumu.onPush('prompt.blocked', (p) => onPromptBlocked(p as unknown as PromptBlockedPush))
  rumu.onPush('llm.delta', (p) => onLlmDelta(p as unknown as LlmDeltaPush))
  rumu.onPush('branch.updated', (p) => onBranchUpdated(p as unknown as BranchUpdatedPush))
  rumu.onPush('session.updated', (p) => onSessionUpdated(p as unknown as SessionUpdatedPush))
  rumu.onPush('decision.requested', (p) => {
    const d = p as unknown as PendingDecision
    if (!state.decisions.some((x) => x.id === d.id)) state.decisions.push(d)
  })
  rumu.onPush('decision.held', (p) => {
    const { id } = p as unknown as DecisionResolvedPush
    state.decisions = state.decisions.map((d) => (d.id === id ? { ...d, expires_at: undefined } : d))
  })
  rumu.onPush('decision.resolved', (p) => {
    const { id } = p as unknown as DecisionResolvedPush
    state.decisions = state.decisions.filter((d) => d.id !== id)
  })
  rumu.onPush('session.deleted', (p) => onSessionDeleted(p as unknown as SessionDeletedPush))
  rumu.onPush('hook.fired', (p) => onHookFired(p as unknown as HookFiredPush))
  rumu.onPush('tool.progress', (p) => onToolProgress(p as unknown as ToolProgressPush))
  rumu.onPush('task.started', (p) => onTaskInfo(p as unknown as TaskInfoPush))
  rumu.onPush('task.done', (p) => onTaskInfo(p as unknown as TaskInfoPush))
  rumu.onPush('task.progress', (p) => onTaskProgress(p as unknown as TaskProgressPush))
  rumu.onPush('turn.started', (p) => onTurnStarted(p as unknown as TurnPush))
  rumu.onPush('turn.done', (p) => onTurnDone(p as unknown as TurnPush))
  rumu.onPush('turn.retrying', (p) => onTurnRetrying(p as unknown as TurnRetryingPush))
  rumu.onPush('turn.error', (p) => onTurnError(p as unknown as TurnErrorPush))
  rumu.onPush('compact.started', (p) => {
    state.compacting[(p as unknown as TurnPush).branch] = true
  })
  rumu.onPush('compact.done', (p) => {
    delete state.compacting[(p as unknown as TurnPush).branch]
  })
  // The turn carries on uncompacted; only the failure is news.
  rumu.onPush('compact.failed', (p) => {
    const { branch, message } = p as unknown as TurnErrorPush
    delete state.compacting[branch]
    toast('error', t('errors.compact'), message)
  })
  rumu.onPush('warning', (p) => onWarning(p as unknown as WarningPush))
  rumu.onPush('settings.updated', (p) => onSettingsUpdated(p as unknown as { settings: SettingsInfo }))
  rumu.onPush('decks.updated', onDecksUpdated)
  rumu.onPush('workspaces.updated', (p) =>
    onWorkspacesUpdated(p as unknown as { workspaces: Workspace[]; projects: Record<string, string[]> }),
  )

  // Updates that landed while the tab was hidden are seen once it shows.
  document.addEventListener('visibilitychange', markSeen)

  // Browser back/forward: show the page the address names, or reopen its
  // session (and view), or land. A page keeps the session beneath it.
  window.addEventListener('popstate', () => {
    state.page = pageFromLocation()
    if (state.page) return
    const id = sessionIdFromLocation()
    state.view = viewFromLocation()
    state.statsRange = rangeFromLocation()
    if (id === state.sessionId) return
    if (id) void openSession(id, { history: 'none' })
    else closeSession()
  })

  rumu.start(async () => {
    await refreshLists()
    void refreshSettings()
    // A reconnect resyncs the open library; boot follows the session URL. A
    // URL naming an unknown session falls back to the landing page.
    if (state.page?.page === 'decks') void refreshLibrary()
    const target = sessionIdFromLocation() ?? state.sessionId
    try {
      if (target && !(await openSession(target, { history: 'none' }))) {
        state.view = 'chat'
        commitRoute('replace')
      }
    } finally {
      state.booting = false
    }
  })
}
