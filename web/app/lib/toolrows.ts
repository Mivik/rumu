// Tool rows, parsed for display: the abbreviated call/result a row shows,
// the full call for its tooltip, status, and the card / sub-agent it
// touches — plus a lone call's phrase ("read store.ts") and which rows are
// file edits (their diffs stand outside an activity run's fold). Shared by
// ToolList (the rows), EditCard and ActivityRun (the summary line).

import { reactive } from 'vue'
import type { Attachment, Branch, FilePatch, Node, TaskStatus } from './types'
import { INTERNAL_WORKSPACE } from './types'
import type { ToolRowInfo } from './store'
import { findAgentChild, state } from './store'
import { taskIdOf, taskLabel } from './tasks'
import { argSummary, preview, workspaceLabel } from './util'
import { renderedContent } from './attachments'
import { diffCounts as foldCounts, diffLines, type DiffCounts, filePatchOf } from './diff'
import { i18n, t } from '../i18n'
import type { MessageKey } from '../i18n'

export type ToolStatus = 'ok' | 'error' | 'blocked'

export interface ToolRowView {
  row: ToolRowInfo
  name: string
  /** The args as displayed (an MCP call's own args). */
  args: unknown
  /** Abbreviated call target for the row ("store.ts", a clipped command). */
  callText: string
  /** Abbreviated result ("12 lines", "exit 1"). */
  resultText: string
  /** The full one-line call summary, for the tooltip. */
  callFull: string
  /** A `bash` call's one-line intent (empty when none). */
  description: string
  output: string
  /** Image attachments of the result (e.g. `read` on an image file). */
  attachments: Attachment[]
  /** The `workspace.file_patch` payload of a write/edit result, if any. */
  patch: FilePatch | null
  /** `+N −M` counts for a write/edit row (null when there is no diff). */
  diff: DiffCounts | null
  status: ToolStatus
  statusLabel: string
  /** Card id from a card read/write call, for the row's CardChip. */
  toolCardId: string | null
  /** The call id of that card use (the drawer opens its exact snapshot). */
  cardUseRef: string | null
  agentChild: Branch | null
  /** Set when this call started a kernel background task. */
  taskId: string | null
  taskStatus: TaskStatus | null
  taskLabel: string | null
}

/** Non-empty lines, for counting entries / output. */
function nonEmptyLines(text: string): string[] {
  return text.split('\n').filter((line) => line.trim())
}

/** The card id in `@rumu:/cards/<id>` / `@rumu:cards/<id>` (optional `.md`). */
function internalCardId(path: string): string | null {
  const prefix = `${INTERNAL_WORKSPACE}:`
  if (!path.startsWith(prefix)) return null
  const rest = path.slice(prefix.length).replace(/^\/?/, '')
  return /^cards\/(.+?)(?:\.md)?$/.exec(rest)?.[1] ?? null
}

/** Line count that ignores the trailing newline of an otherwise exact file. */
function lineCount(text: string): number {
  if (!text) return 0
  return text.split('\n').length - (text.endsWith('\n') ? 1 : 0)
}

/** A multi-line command on one line: `a\n  b` → `a ; b`. */
function oneLine(command: string): string {
  return command
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .join(' ; ')
}

/** A path's last segment: "src/lib/store.ts" → "store.ts". */
function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  return trimmed.slice(trimmed.lastIndexOf('/') + 1) || path
}

type Args = Record<string, unknown> | null
type Call = { name: string; args: unknown } | null

/** A call's args as a record, if they are an object. */
export function argsOf(call: { args: unknown } | null): Args {
  return call?.args && typeof call.args === 'object' && !Array.isArray(call.args)
    ? (call.args as Record<string, unknown>)
    : null
}

/** A call as the user reads it: an `invoke` stands for the action it runs,
 * named by its full name — `<type>.<action>` for rumu's (`task.kill`),
 * `<server>/<tool>` for an MCP tool; neither clashes with a built-in — with
 * the action's own args, and the object it acts on as their `path`. */
export function displayCall<C extends { name: string; args: unknown }>(call: C): C
export function displayCall<C extends { name: string; args: unknown }>(call: C | null): C | null
export function displayCall<C extends { name: string; args: unknown }>(call: C | null): C | null {
  if (call?.name !== 'invoke') return call
  const args = argsOf(call)
  if (typeof args?.action !== 'string' || !args.action) return call
  const own = argsOf({ args: args.args }) ?? {}
  const shown = typeof args.path === 'string' ? { ...own, path: args.path } : own
  return { ...call, name: args.action, args: shown }
}

/** Whether a call (as displayed) is an action of an `@rumu` object. */
function isAction(name: string): boolean {
  return name.includes('.') && !name.includes('/')
}

function strArg(args: Args, key: string): string {
  return args && typeof args[key] === 'string' ? (args[key] as string).trim() : ''
}

/** The call's full one-line summary (tooltip). File tools show the `path`
 * verbatim (it may carry a `<ws>:` prefix); `bash` names its workspace. */
function callFull(name: string, call: Call): string {
  if (!call) return ''
  const args = argsOf(call)
  const str = (key: string): string => strArg(args, key)
  switch (name) {
    case 'read':
    case 'write':
    case 'edit':
    case 'delete':
    case 'list':
    case 'show':
      // `proj:` / `@rumu:` (empty rest) is that workspace's root.
      return str('path') || (name === 'list' ? 'root' : '')
    case 'bash': {
      const command = preview(oneLine(str('command')), 200)
      const ws = str('workspace')
      if (command) return ws ? `${ws}: ${command}` : command
      return ws
    }
    case 'workspace.use':
      return workspaceLabel(baseName(str('path')))
    case 'search': {
      const query = str('query')
      const scope = str('scope')
      return scope ? `${query} (in ${scope})` : query
    }
    case 'code':
      return preview(str('code'), 200)
    case 'agent':
      return str('task')
    default:
      return isAction(name) ? str('path') : argSummary(call)
  }
}

/** The call's abbreviated target: a basename, a clipped first line. */
function callShort(name: string, call: Call): string {
  if (!call) return ''
  const args = argsOf(call)
  const str = (key: string): string => strArg(args, key)
  switch (name) {
    case 'read':
    case 'write':
    case 'edit':
    case 'delete':
    case 'show':
      return str('path') ? baseName(str('path')) : ''
    case 'list': {
      // `proj:` / `@scratch:` (a workspace's root) shows as written: the
      // colon is the address, `@scratch/` would name a directory.
      const path = str('path')
      if (path.endsWith(':')) return path
      if (path && path !== '.' && path !== '/') return `${baseName(path)}/`
      return '/'
    }
    case 'bash':
      // the row's `$` line: as much as fits (the row clips it)
      return oneLine(str('command'))
    case 'code':
      return preview(str('code'), 40)
    case 'workspace.use':
      return workspaceLabel(baseName(str('path')))
    case 'search': {
      // A scope is always under `@rumu:`; the row keeps only the path.
      const query = preview(str('query'), 40)
      const scope = str('scope').replace(/^@rumu:/, '')
      return scope ? `${query} (in ${scope})` : query
    }
    case 'agent':
      return preview(str('task'), 40)
    default:
      // An action's object: the last part of its path (`t-1a2b` for a task).
      return isAction(name) ? baseName(str('path')) : preview(argSummary(call), 40)
  }
}

/** `+N −M` counts for a write/edit/delete row: from the result's hunks, or
 * — for results older than the payload — a browser diff of the call's
 * strings. */
function diffOf(name: string, patch: FilePatch | null, args: Args): DiffCounts | null {
  if (name !== 'write' && name !== 'edit' && name !== 'delete') return null
  if (patch?.hunks) {
    let added = 0
    let removed = 0
    for (const hunk of patch.hunks) {
      for (const line of hunk.lines) {
        if (line.startsWith('+')) added++
        else if (line.startsWith('-')) removed++
      }
    }
    return added + removed ? { added, removed } : null
  }
  // An update or delete whose hunks were dropped (too big, non-UTF-8):
  // nothing to count.
  if (patch?.kind === 'update' || name === 'delete') return null
  // No payload: a legacy edit diffs its strings, a legacy write / a create
  // counts its content as all-added.
  const str = (key: string): string => (typeof args?.[key] === 'string' ? (args[key] as string) : '')
  const counts =
    name === 'edit'
      ? foldCounts(diffLines(str('old_string'), str('new_string')))
      : { added: lineCount(str('content')), removed: 0 }
  return counts.added + counts.removed ? counts : null
}

/** A `bash` result split into its `exit code: N` header and the output
 * after it; `code` is null when there is no header (a tool error, a block). */
export function bashExit(text: string): { code: number | null; body: string } {
  const m = /^exit code: (-?\d+)\n?/.exec(text)
  return m ? { code: Number(m[1]), body: text.slice(m[0].length) } : { code: null, body: text }
}

/** A foreground bash call's timeout in seconds (past it the kernel moves the
 * call to the background; 60 when not given), or null for a background call. */
export function bashTimeout(args: Args): number | null {
  if (args?.background === true) return null
  const t = args?.timeout
  return typeof t === 'number' && t > 0 ? Math.min(t, 600) : 60
}

/** A bash call's args other than its command and intent, for its opened row. */
export function bashExtraArgs(args: Args): [string, unknown][] {
  return Object.entries(args ?? {}).filter(([k]) => k !== 'command' && k !== 'description')
}

/** `+N −M`, leaving out a zero side (a create says `+N`, a delete `−M`). */
function diffText(diff: DiffCounts): string {
  const parts = [diff.added ? `+${diff.added}` : '', diff.removed ? `−${diff.removed}` : '']
  return parts.filter(Boolean).join(' ')
}

/** What the call produced, as a count or a word (empty when the call line says it). */
function resultShort(name: string, output: string, diff: DiffCounts | null): string {
  if (!output) return ''
  switch (name) {
    case 'workspace.use':
      return ''
    case 'list': {
      const n = nonEmptyLines(output).length
      return n ? t('tools.count.entries', { count: n }) : t('tools.result.empty')
    }
    case 'read':
      return t('tools.count.lines', { count: lineCount(output) })
    case 'write':
    case 'edit':
    case 'delete':
      // the row shows these colored; the plain form serves the tooltip
      return diff ? diffText(diff) : ''
    case 'bash': {
      // a clean exit is the norm; say the exit code only when it isn't 0
      const { code, body } = bashExit(output)
      if (code) return t('tools.result.exit', { code })
      const lines = nonEmptyLines(body).length
      return lines ? t('tools.count.lines', { count: lines }) : ''
    }
    default: {
      const lines = nonEmptyLines(output)
      if (lines.length > 1) return t('tools.count.lines', { count: lines.length })
      return preview(lines[0] ?? '', 32)
    }
  }
}

interface ParsedRow {
  /** Locale the translated parts were built for (a switch invalidates them). */
  locale: string
  callShort: string
  callFull: string
  result: string
  diff: DiffCounts | null
}

/** Parsed JSON outputs, per result node: parsed when a row first opens. */
const jsonCache = new WeakMap<Node, object | null>()

/** An ok output that is a JSON object or array (an MCP tool's usual
 * answer), for the opened row to show as a tree. */
export function outputJson(v: ToolRowView): object | null {
  if (v.status !== 'ok') return null
  let json = jsonCache.get(v.row.result)
  if (json === undefined) {
    json = parseJson(v.output)
    jsonCache.set(v.row.result, json)
  }
  return json
}

function parseJson(text: string): object | null {
  const s = text.trim()
  if (!s.startsWith('{') && !s.startsWith('[')) return null
  try {
    const v: unknown = JSON.parse(s)
    return v && typeof v === 'object' ? v : null
  } catch {
    return null
  }
}

/** Parsed summaries per immutable result node (the timeline recomputes on deltas). */
const parsedCache = new WeakMap<Node, ParsedRow>()

const BLOCKED_PREFIX = 'Blocked by hook: '

/** The hook's block reason, when a tool result was blocked. */
function blockedReasonOf(text: string): string | null {
  return text.startsWith(BLOCKED_PREFIX) ? text.slice(BLOCKED_PREFIX.length).trim() : null
}

export function toolRowView(row: ToolRowInfo): ToolRowView {
  const content = row.result.body.type === 'tool_result' ? row.result.body.content : undefined
  // The output is read as text: images appear as their placeholders.
  const output = renderedContent(content)
  const attachments = content?.attachments ?? []
  const isError = row.result.body.type === 'tool_result' && row.result.body.is_error
  const blocked = isError ? blockedReasonOf(output) : null
  const call = displayCall(row.call)
  const name = call?.name ?? 'tool'
  // A card tool's target id, for its row chip: the current read/write/edit
  // whose path addresses `@rumu:/cards/<id>` (qualified always; `/cards/<id>`
  // and `cards/<id>` when `@rumu` is the branch default).
  const args = argsOf(call)
  let toolCardId: string | null = null
  if (args && (name === 'read' || name === 'write' || name === 'edit')) {
    const path = typeof args.path === 'string' ? args.path : null
    if (path) {
      const internal = internalCardId(path)
      const bare = /^(?:\/)?cards\/(.+?)(?:\.md)?$/.exec(path)
      if (internal) toolCardId = internal
      else if (bare && state.pathState?.default_workspace === INTERNAL_WORKSPACE) toolCardId = bare[1]!
    }
  }
  // The result node is immutable: parse its output once, not on every delta.
  // The translated result text is rebuilt when the locale changes.
  const locale = i18n.global.locale.value
  const patch = filePatchOf(content)
  let parsed = parsedCache.get(row.result)
  if (!parsed || parsed.locale !== locale) {
    const diff = diffOf(name, patch, args)
    parsed = {
      locale,
      callShort: callShort(name, call),
      callFull: callFull(name, call),
      diff,
      // A blocked row is already explained by its stamp; an error keeps a
      // clipped first line so the row hints at what went wrong (a bash
      // that ran says its exit code).
      result:
        blocked !== null
          ? ''
          : isError && !(name === 'bash' && bashExit(output).code !== null)
            ? preview(nonEmptyLines(output)[0] ?? '', 40)
            : resultShort(name, output, diff),
    }
    parsedCache.set(row.result, parsed)
  }
  const callId = row.result.body.type === 'tool_result' ? row.result.body.call_id : ''
  let agentChild = name === 'agent' ? findAgentChild(callId) : null
  // A background call's result is the immediate "started task <id>" line; the
  // task table carries its live status and (for agents) the child branch.
  const taskId = taskIdOf(output)
  const task = taskId ? state.tasks[taskId] : undefined
  if (!agentChild && task?.kind.type === 'agent' && task.kind.child_branch) {
    agentChild = state.branches[task.kind.child_branch] ?? null
  }
  return {
    row,
    name,
    args: call?.args,
    callText: parsed.callShort,
    resultText: parsed.result,
    callFull: parsed.callFull,
    description: name === 'bash' ? strArg(args, 'description') : '',
    output: blocked ?? output,
    attachments,
    patch,
    diff: parsed.diff,
    status: blocked !== null ? 'blocked' : isError ? 'error' : 'ok',
    statusLabel:
      blocked !== null
        ? t('tools.row.status.blocked')
        : isError
          ? t('tools.row.status.error')
          : t('tools.row.status.ok'),
    toolCardId,
    cardUseRef: toolCardId && callId ? callId : null,
    agentChild,
    taskId: taskId ?? null,
    taskStatus: task?.status ?? null,
    taskLabel: task ? taskLabel(task) : null,
  }
}

/** A row's full facts in one line, for tooltips. */
export function toolRowTitle(v: ToolRowView): string {
  const facts = [v.callFull, v.resultText].filter(Boolean).join(' · ')
  return facts ? `${v.name} ${facts} → ${v.statusLabel}` : `${v.name} → ${v.statusLabel}`
}

// ---------------------------------------------------------------------------
// Run summary

/** Verb key for each call kind with a target to name (see tools.json). */
const VERB_KEYS: Record<string, string> = {
  read: 'read',
  write: 'write',
  edit: 'edit',
  delete: 'delete',
  'read:card': 'readCard',
  'write:card': 'writeCard',
  'edit:card': 'editCard',
  list: 'list',
  search: 'search',
  show: 'show',
  'task.kill': 'kill',
  'workspace.use': 'workspaceUse',
  'deck.mount': 'mount',
  'deck.unmount': 'unmount',
}

/** A piece of a summary line; `mono` marks a literal (a file, a command). */
export interface PhrasePart {
  text: string
  mono?: boolean
}

/** A lone call's phrase: its intent, or its verb and target ("read
 * store.ts"); a call with neither names the tool. */
export function toolPhrase(v: ToolRowView): PhrasePart[] {
  if (v.description) return [{ text: v.description }]
  const key = v.toolCardId ? `${v.name}:card` : v.name
  const verbKey = VERB_KEYS[key]
  if (verbKey && v.callText) {
    const verb = t(`tools.verb.${verbKey}` as MessageKey)
    return [{ text: `${verb} ` }, { text: v.callText, mono: true }]
  }
  return [{ text: `${t('tools.verb.other')} ` }, { text: v.name, mono: true }]
}

/** A successful edit of a workspace file or a card: its diff stands outside
 * the run's fold, as an EditCard. A deleted card stays a row (its patch
 * names no file); a failed edit is a row with its error. */
export function isFileEdit(v: ToolRowView): boolean {
  if (v.status !== 'ok' || v.row.call === null) return false
  if (v.name === 'delete') return !!v.patch?.file
  return v.name === 'write' || v.name === 'edit'
}

/** Rows the user expanded, and blocked rows they collapsed (blocked rows
 * start expanded), by result node id. Shared: a `code` row's edits stand
 * outside it only while it is shut (ActivityRun), and a box re-split around
 * them keeps its rows' state. */
const expandedRows = reactive(new Set<string>())
const collapsedRows = reactive(new Set<string>())

export function isRowOpen(v: ToolRowView): boolean {
  const id = v.row.result.id
  if (collapsedRows.has(id)) return false
  return v.status === 'blocked' || expandedRows.has(id)
}

export function toggleRow(v: ToolRowView): void {
  const id = v.row.result.id
  if (v.status === 'blocked' && !expandedRows.has(id)) collapsedRows.add(id)
  else if (expandedRows.has(id)) expandedRows.delete(id)
  else expandedRows.add(id)
}

/** The file edits a row stands for: itself, or those its script's calls
 * made (a blocked invoke changed nothing). */
export function fileEditsOf(row: ToolRowInfo): ToolRowView[] {
  const ran = row.invokes.filter(
    (r) => r.result.body.type === 'tool_result' && !r.result.body.invoke?.blocked,
  )
  return [row, ...ran].map(toolRowView).filter(isFileEdit)
}
