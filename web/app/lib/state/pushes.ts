// WebSocket push handling and boot. This is the only module that translates
// server pushes into state mutations; it calls actions from the sibling
// modules rather than mutating anything itself (beyond its own slices).

import { rumu } from '../api'
import { routeSession, sessionIdFromLocation } from '../router'
import type {
  BranchUpdatedPush,
  CodeCallPush,
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
import {
  closeSession,
  forgetSession,
  markSeen,
  openSession,
  refreshAssistants,
  refreshCards,
  refreshLists,
} from './sessions'
import { refreshLibrary } from './library'
import { loadPath } from './tree'
import { toast } from './ui'

let traceSeq = 1
let queuedSeq = 1

/** Live tool/task output kept in memory (chars). */
const MAX_LIVE_OUTPUT = 1_000_000

/** Did this node change the card library on disk? A write/edit records a
 * `card_use`; the memory hook records `card_update` notices for cards that
 * changed elsewhere. Both make `state.cards` stale until re-read. */
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

/** Did this node change the branch's workspaces (attached or default)? */
function workspaceChanged(node: Node): boolean {
  const tag = node.body.type === 'note' ? node.body.content.data?.tag : undefined
  return tag === 'workspace.use' || tag === 'workspace.set'
}

/** `workspaces.updated`: the registry changed. A removed workspace also left
 *  the assistants, and drops out of the open branch. */
function onWorkspacesUpdated(p: { workspaces: Workspace[] }): void {
  state.workspaces = p.workspaces
  void refreshAssistants()
  if (state.activeBranch) void loadPath(state.activeBranch)
}

function onNodeAppended(p: NodeAppendedPush): void {
  const { branch, node } = p
  if (branch === state.activeBranch && workspaceChanged(node)) void loadPath(branch)
  if (cardChanged(node)) {
    void refreshCards()
    // The model may be writing cards while the user browses the library.
    if (state.library.isOpen) void refreshLibrary()
  }
  if (state.sessionId && node.session === state.sessionId) {
    state.nodes[node.id] = node
    // If the node extends the active branch's head, the path grows live.
    const b = state.branches[branch]
    if (b && branch === state.activeBranch && node.parent === b.head) {
      b.head = node.id
    }
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
      const i = queued.findIndex((q) => q.text === node.body.content.text)
      if (i >= 0) queued.splice(i, 1)
      else queued.shift()
      if (!queued.length) delete state.queuedPrompts[branch]
    }
  }
}

/** A prompt accepted while the branch was busy; shown pending until written. */
function onPromptQueued(p: PromptQueuedPush): void {
  const list = state.queuedPrompts[p.branch] ?? (state.queuedPrompts[p.branch] = [])
  list.push({ id: queuedSeq++, text: p.text })
}

function onLlmDelta(p: LlmDeltaPush): void {
  // The attempt is live again; its retry notice stops applying.
  delete state.retrying[p.branch]
  applyDelta(p)
}

/** A foreground tool's live output (used by the running-tool rows). */
function onToolProgress(p: ToolProgressPush): void {
  const key = `${p.branch}:${p.call_id}`
  let buf = state.toolProgress[key]
  if (!buf) {
    buf = { branch: p.branch, call_id: p.call_id, text: '', data: null }
    state.toolProgress[key] = buf
  }
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
    state.branches[p.branch.id] = p.branch
  }
}

function onSessionUpdated(p: SessionUpdatedPush): void {
  const s = p.session
  const i = state.sessions.findIndex((x) => x.id === s.id)
  if (i >= 0) state.sessions[i] = s
  else state.sessions.push(s)
  if (state.sessionId === s.id) {
    state.session = s
    markSeen()
    if (
      s.active_branch !== state.activeBranch &&
      state.branches[s.active_branch] &&
      !state.busy[state.activeBranch ?? '']
    ) {
      state.activeBranch = s.active_branch
      loadPath(s.active_branch)
    }
  }
}

/** Another tab deleted a session; this one drops it (and leaves it if open). */
function onSessionDeleted(p: SessionDeletedPush): void {
  forgetSession(p.session.id)
}

function onTurnStarted(p: TurnPush): void {
  state.busy[p.branch] = true
  delete state.retrying[p.branch]
  delete state.failedTurns[p.branch]
}

function onTurnDone(p: TurnPush): void {
  state.busy[p.branch] = false
  delete state.deltas[p.branch]
  delete state.retrying[p.branch]
  delete state.failedTurns[p.branch]
}

function onTurnError(p: TurnErrorPush): void {
  state.busy[p.branch] = false
  delete state.deltas[p.branch]
  delete state.retrying[p.branch]
  // A hook refused the prompt before the turn started: nothing was recorded
  // and no reply is owed — a toast, never a failure row.
  if (p.blocked) {
    toast('error', p.message)
    return
  }
  // Shown once, live, where the missing reply would be (see `owedReply`) —
  // never rebuilt from the stored marker on a reload. A branch that is not on
  // screen has no row to land in, so its failure still needs a toast.
  state.failedTurns[p.branch] = p.message
  if (p.branch !== state.activeBranch) toast('error', p.message)
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

/** A tool call from inside a `code` script finished. */
function onCodeCall(p: CodeCallPush): void {
  const known = state.branches[p.branch]
  if (known) {
    if (known.session !== state.sessionId) return
  } else if (!state.sessionId || !Object.values(state.busy).some(Boolean)) {
    return
  }
  const list = state.codeCalls[p.parent_call] ?? (state.codeCalls[p.parent_call] = [])
  const i = list.findIndex((c) => c.seq === p.seq)
  if (i >= 0) list[i] = p
  else {
    list.push(p)
    list.sort((a, b) => a.seq - b.seq)
  }
}

let started = false

export function startApp(): void {
  if (started) return
  started = true

  // A refresh on `/s/<id>` names its session in the address. From the first
  // paint, hold the chat pane for it instead of flashing the landing page
  // while the boot requests are in flight.
  state.booting = sessionIdFromLocation() !== null

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
  rumu.onPush('decision.resolved', (p) => {
    const { id } = p as unknown as DecisionResolvedPush
    state.decisions = state.decisions.filter((d) => d.id !== id)
  })
  rumu.onPush('session.deleted', (p) => onSessionDeleted(p as unknown as SessionDeletedPush))
  rumu.onPush('hook.fired', (p) => onHookFired(p as unknown as HookFiredPush))
  rumu.onPush('code.call', (p) => onCodeCall(p as unknown as CodeCallPush))
  rumu.onPush('tool.progress', (p) => onToolProgress(p as unknown as ToolProgressPush))
  rumu.onPush('task.started', (p) => onTaskInfo(p as unknown as TaskInfoPush))
  rumu.onPush('task.done', (p) => onTaskInfo(p as unknown as TaskInfoPush))
  rumu.onPush('task.progress', (p) => onTaskProgress(p as unknown as TaskProgressPush))
  rumu.onPush('turn.started', (p) => onTurnStarted(p as unknown as TurnPush))
  rumu.onPush('turn.done', (p) => onTurnDone(p as unknown as TurnPush))
  rumu.onPush('turn.retrying', (p) => onTurnRetrying(p as unknown as TurnRetryingPush))
  rumu.onPush('turn.error', (p) => onTurnError(p as unknown as TurnErrorPush))
  rumu.onPush('warning', (p) => onWarning(p as unknown as WarningPush))
  rumu.onPush('workspaces.updated', (p) => onWorkspacesUpdated(p as unknown as { workspaces: Workspace[] }))

  // Updates that landed while the tab was hidden are seen once it shows.
  document.addEventListener('visibilitychange', markSeen)

  // Browser back/forward: reopen the session the address names, or land.
  window.addEventListener('popstate', () => {
    const id = sessionIdFromLocation()
    if (id === state.sessionId) return
    if (id) void openSession(id, { history: 'none' })
    else closeSession()
  })

  rumu.start(async () => {
    await refreshLists()
    // A reconnect resyncs the open library; boot follows the session URL. A
    // URL naming an unknown session falls back to the landing page.
    if (state.library.isOpen) void refreshLibrary()
    const target = sessionIdFromLocation() ?? state.sessionId
    try {
      if (target && !(await openSession(target, { history: 'none' }))) {
        routeSession(null, 'replace')
      }
    } finally {
      state.booting = false
    }
  })
}
