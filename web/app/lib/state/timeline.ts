// Timeline grouping: the chat renders grouped entries, not raw nodes — a user
// message, then ONE assistant group per turn (assistant → tool results →
// assistant …) with a single header, plus "context" strips collapsing
// consecutive notes/markers. Inside a group, the work between two pieces of
// visible text — each step's reasoning and its tool results, and while the
// turn runs its calls still waiting on a result — is one "activity" run,
// open while it is the frontier and folded into a summary line after
// ("Worked for 3m 12s · 27 calls"); only answer text and context pills stand
// on their own.

import type { Branch, BranchId, Node, ToolCall } from '../types'
import type { Compaction } from '../generated/Compaction'
import { CATALOG_TAG, TURN_ERROR_TAG, reasoningText } from '../types'
import { contentText } from '../util'
import { artifactOf } from '../artifacts'
import type { callsOnPath } from './derived'
import { state } from './state'

/** Injected at `llm.pre`, so it belongs to the assistant turn, not the user row. */
function isCatalogNote(n: Node): boolean {
  return n.body.type === 'note' && n.body.content.data?.tag === CATALOG_TAG
}

/** Cards recalled for the prompt (`card.import`): rumu's doing, so they fold
 *  into the work of the step they were recalled for. */
function isRecallNote(n: Node): boolean {
  return n.body.type === 'note' && n.body.source === 'memory' && n.body.content.data?.tag === 'card.import'
}

/** A bookkeeping marker with nothing to show: the root marker, the
 * `card.use` record a card read/write leaves after its tool result (the
 * Context data reads it from the path; the tool row already shows the card),
 * and a `turn.error` failure record (the error row at the end shows an
 * unanswered one; in the transcript it leaves no trace), a `workspace.set`
 * (the composer's workspace chip shows the result), the empty
 * `card.import` recording a pinned card the introduction carried, and the
 * `model.use` / `model.reset` / `card.mount` / `card.unmount` state
 * markers (the composer and the tool row show those).
 * Kept out of the timeline entirely — as an empty context item it would
 * split the activity run around it in two. */
function isSilentMarker(n: Node): boolean {
  if (n.body.type !== 'note') return false
  const tag = n.body.content.data?.tag
  if (tag === 'workspace.set') return true
  if (n.body.content.text !== '') return false
  return (
    tag === 'session.root' ||
    tag === 'card.use' ||
    tag === 'card.import' ||
    tag === TURN_ERROR_TAG ||
    tag === 'model.use' ||
    tag === 'model.reset' ||
    tag === 'card.mount' ||
    tag === 'card.unmount'
  )
}

export interface ToolRowInfo {
  result: Node
  call: { call_id: string; name: string; args: unknown } | null
  assistant: Node | null
  /** The calls a plugin (a `code` script) made through this one, in order. */
  invokes: ToolRowInfo[]
}

/** An assistant step's answer text (`node: null` = the live stream's). */
export interface GroupTextItem {
  kind: 'text'
  node: Node | null
  streaming?: boolean
}
/** A step's reasoning (`node: null` = the live stream's). */
export interface ActivityThought {
  kind: 'thought'
  node: Node | null
  streaming?: boolean
}
/** Consecutive tool results. */
export interface ActivityTools {
  kind: 'tools'
  rows: ToolRowInfo[]
}
/** The card catalog handed to the model before a step: it reads as part of
 * the work, so it folds into the run with that step's reasoning. */
export interface ActivityCatalog {
  kind: 'catalog'
  node: Node
}
/** Cards recalled before a step: they fold into the run, whose body lists
 *  them. */
export interface ActivityRecall {
  kind: 'recall'
  nodes: Node[]
}
/** The live turn's calls still waiting on their result, each with the
 *  assistant node that made it (its `at` is when the call started) and the
 *  calls a `code` script made through it so far. */
export interface ActivityRunning {
  kind: 'running'
  calls: { call: ToolCall; assistant: Node; invokes: ToolRowInfo[] }[]
}
export type ActivityStep =
  ActivityThought | ActivityTools | ActivityCatalog | ActivityRecall | ActivityRunning
export interface GroupActivityItem {
  kind: 'activity'
  steps: ActivityStep[]
}
export interface GroupContextItem {
  kind: 'context'
  nodes: Node[]
}
/** This step ran without the reasoning before it: the provider could no
 * longer verify it, so it is never sent again. */
export interface GroupResetItem {
  kind: 'reset'
  node: Node
}
/** A file the model showed the user (a `show` result): its own card in the
 *  turn, not a folded tool row. */
export interface GroupArtifactItem {
  kind: 'artifact'
  node: Node
}
export type GroupItem =
  GroupTextItem | GroupActivityItem | GroupContextItem | GroupResetItem | GroupArtifactItem

/** Append a step to the group's trailing activity run (opening one if needed). */
function pushActivity(g: TimelineGroup, step: ActivityStep): void {
  const last = g.items[g.items.length - 1]
  if (last && last.kind === 'activity') last.steps.push(step)
  else g.items.push({ kind: 'activity', steps: [step] })
}

/** Add a recalled card to the trailing recall step, or start one. */
function pushRecall(g: TimelineGroup, n: Node): void {
  const last = g.items[g.items.length - 1]
  const step = last?.kind === 'activity' ? last.steps[last.steps.length - 1] : undefined
  if (step?.kind === 'recall') step.nodes.push(n)
  else pushActivity(g, { kind: 'recall', nodes: [n] })
}

/** Move the notes collected for a turn into its group: the plain ones (a card
 * update, a task completion, …) as a context line of their own, then each
 * recalled card and card catalog as activity steps, so the reasoning that
 * follows joins its fold. */
function moveNotes(g: TimelineGroup, notes: Node[]): void {
  const plain = notes.filter((n) => !isCatalogNote(n) && !isRecallNote(n))
  if (plain.length) g.items.push({ kind: 'context', nodes: plain })
  for (const n of notes) {
    if (isRecallNote(n)) pushRecall(g, n)
    else if (isCatalogNote(n)) pushActivity(g, { kind: 'catalog', node: n })
  }
}

export interface TimelineGroup {
  kind: 'group'
  items: GroupItem[]
  assistants: Node[]
  /** Last assistant node that is a safe fork point, if any. */
  safeNode: Node | null
}
export interface TimelineUserEntry {
  kind: 'user'
  node: Node
  /** Notes/markers injected right after this prompt (memory imports, notes). */
  contextNodes: Node[]
}
export interface TimelineContextEntry {
  kind: 'context'
  nodes: Node[]
}
/** A compaction, where it happened: from here on the model sees its summary
 *  in place of the older messages. */
export interface TimelineCompactionEntry {
  kind: 'compaction'
  node: Node
}
export type TimelineEntry = TimelineUserEntry | TimelineContextEntry | TimelineGroup | TimelineCompactionEntry

/** The compaction payload of a note, if it is one. */
export function compactionOf(n: Node): Compaction | null {
  if (n.body.type !== 'note' || n.body.content.data?.tag !== 'branch.compaction') return null
  return n.body.content.data.data as Compaction
}

type CallsOnPath = typeof callsOnPath.value

/** The nodes a conversation shows of branch `b` (`path` its path): all
 *  of it, or of a sub-agent its own part from its goal prompt on, the prompt
 *  showing the task as it was given (the kernel's sub-agent framing is the
 *  model's, not the reader's; what was seeded before it is the parent's). */
export function shownOf(b: Branch | null, path: Node[]): Node[] {
  if (b?.kind.type !== 'agent') return path
  const task = b.kind.task
  const own = path.slice(path.findIndex((n) => n.id === b.forked_from) + 1)
  const at = own.findIndex((n) => n.body.type === 'user')
  const prompt = own[at]
  if (!prompt || prompt.body.type !== 'user') return own
  const shown: Node = { ...prompt, body: { ...prompt.body, content: { ...prompt.body.content, text: task } } }
  return [shown, ...own.slice(at + 1)]
}

/** Group `nodes` (a branch's path, or the part of it to show) into timeline
 *  entries, with `branch`'s live stream attached. */
export function buildTimeline(nodes: Node[], calls: CallsOnPath, branch: BranchId | null): TimelineEntry[] {
  const out: TimelineEntry[] = []
  const answered = new Set<string>()
  const invokes = new Map<string, ToolRowInfo[]>()
  for (const n of nodes) {
    if (n.body.type !== 'tool_result') continue
    answered.add(n.body.call_id)
    const inv = n.body.invoke
    if (!inv) continue
    const call = { call_id: n.body.call_id, name: inv.name, args: inv.args }
    const list = invokes.get(inv.parent) ?? []
    list.push({ result: n, call, assistant: null, invokes: [] })
    invokes.set(inv.parent, list)
  }

  let group: TimelineGroup | null = null
  let ctx: Node[] = []
  let userEntry: TimelineUserEntry | null = null

  const flushCtx = (): void => {
    if (ctx.length) {
      out.push({ kind: 'context', nodes: ctx })
      ctx = []
    }
  }
  const ensureGroup = (): TimelineGroup => {
    if (!group) {
      userEntry = null
      group = { kind: 'group', items: [], assistants: [], safeNode: null }
      out.push(group)
      // Notes collected for this turn (e.g. the card catalog injected at
      // `llm.pre`) sit inside the group, after the mascot head.
      if (ctx.length) {
        moveNotes(group, ctx)
        ctx = []
      }
    }
    return group
  }

  for (const n of nodes) {
    const b = n.body
    // A compaction is a line of its own, where it happened.
    if (compactionOf(n)) {
      flushCtx()
      group = null
      userEntry = null
      out.push({ kind: 'compaction', node: n })
      continue
    }
    if (b.type === 'user') {
      flushCtx()
      group = null
      userEntry = { kind: 'user', node: n, contextNodes: [] }
      out.push(userEntry)
    } else if (b.type === 'note') {
      if (isSilentMarker(n)) continue
      // A widget the user saved: its card joins the reply it came from.
      const g = group as TimelineGroup | null
      if (artifactOf(b.content) && g) {
        g.items.push({ kind: 'artifact', node: n })
        continue
      }
      if (isCatalogNote(n) || isRecallNote(n)) ctx.push(n)
      else if (userEntry) userEntry.contextNodes.push(n)
      else ctx.push(n)
    } else if (b.type === 'assistant') {
      const g = ensureGroup()
      if (ctx.length) {
        moveNotes(g, ctx)
        ctx = []
      }
      if (b.reasoning_reset) g.items.push({ kind: 'reset', node: n })
      // Reasoning joins the activity run; a step that only calls tools adds
      // nothing visible, so the tool rows around it stay one run.
      if (reasoningText(b.reasoning)) pushActivity(g, { kind: 'thought', node: n })
      if (contentText(b.content).trim()) g.items.push({ kind: 'text', node: n })
      g.assistants.push(n)
      const pending = b.tool_calls.some((c) => !answered.has(c.call_id))
      if (!pending) g.safeNode = n
    } else if (b.type === 'tool_result') {
      // A plugin invoke's result is trace data, nested under its enclosing
      // call by CodeCalls — never a top-level row.
      if (b.invoke) continue
      const g = ensureGroup()
      if (ctx.length) {
        moveNotes(g, ctx)
        ctx = []
      }
      if (!b.is_error && artifactOf(b.content)) {
        g.items.push({ kind: 'artifact', node: n })
        continue
      }
      const paired = calls.get(b.call_id) ?? null
      const row: ToolRowInfo = {
        result: n,
        call: paired?.call ?? null,
        assistant: paired?.node ?? null,
        invokes: invokes.get(b.call_id) ?? [],
      }
      const last = g.items[g.items.length - 1]
      const step = last?.kind === 'activity' ? last.steps[last.steps.length - 1] : undefined
      if (step?.kind === 'tools') step.rows.push(row)
      else pushActivity(g, { kind: 'tools', rows: [row] })
    }
  }
  // Attach the live stream to the group it belongs to (single header per turn).
  // The group exists from the moment the prompt is accepted, before the first
  // token: the mascot shows up immediately and waits.
  const delta = branch ? state.deltas[branch] : undefined
  let live: TimelineGroup | null = null
  if (branch && state.busy[branch]) {
    const last = out[out.length - 1]
    if (last && last.kind === 'group') live = last
    else if (last && last.kind === 'user') {
      live = { kind: 'group', items: [], assistants: [], safeNode: null }
      out.push(live)
    }
  }
  // Notes queued for this turn (the catalog injected at `llm.pre`) belong
  // inside the live group, after the mascot. Without a live group they stay
  // a standalone context row.
  if (ctx.length) {
    if (live) {
      moveNotes(live, ctx)
      ctx = []
    } else {
      flushCtx()
    }
  }
  // The calls still running close the run they belong to: the frontier.
  if (live) {
    const running = live.assistants.flatMap((a) =>
      a.body.type === 'assistant'
        ? a.body.tool_calls
            .filter((c) => !answered.has(c.call_id))
            .map((call) => ({ call, assistant: a, invokes: invokes.get(call.call_id) ?? [] }))
        : [],
    )
    if (running.length) pushActivity(live, { kind: 'running', calls: running })
  }
  if (live && delta?.hasReasoning) pushActivity(live, { kind: 'thought', node: null, streaming: true })
  if (live && delta?.hasText) live.items.push({ kind: 'text', node: null, streaming: true })
  // No user prompt on the path yet (a brand-new session mid-request):
  // nothing to hang the group under, so leave the timeline alone.
  return out
}
