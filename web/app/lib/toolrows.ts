// Tool rows, parsed for display: the abbreviated call/result a row shows,
// the full call for its tooltip, status, and the card / sub-agent it
// touches — plus the counted phrases ("read 3 files", "ran 2 commands") an
// activity run folds them into. Shared by ToolList (the rows) and
// ActivityRun (the summary line).

import type { Attachment, Branch, Node, TaskStatus } from './types'
import { INTERNAL_WORKSPACE } from './types'
import type { ToolRowInfo } from './store'
import { findAgentChild, state } from './store'
import { taskIdOf, taskLabel } from './tasks'
import { argSummary, preview } from './util'
import { renderedContent } from './attachments'
import { i18n, t } from '../i18n'
import type { MessageKey } from '../i18n'

export type ToolStatus = 'ok' | 'error' | 'blocked'

export interface ToolRowView {
  row: ToolRowInfo
  name: string
  /** Abbreviated call target for the row ("store.ts", a clipped command). */
  callText: string
  /** Abbreviated result ("12 lines", "exit 1"). */
  resultText: string
  /** The full one-line call summary, for the tooltip. */
  callFull: string
  output: string
  /** Image attachments of the result (e.g. `read` on an image file). */
  attachments: Attachment[]
  status: ToolStatus
  statusLabel: string
  /** Card id from a card read/write call, for the row's CardChip. */
  toolCardId: string | null
  /** The call id of that card use (the drawer opens its exact snapshot). */
  cardUseRef: string | null
  agentChild: Branch | null
  /** `agent` rows: the sub-agent asks for a clean context. */
  agentNoContext: boolean
  /** `agent` rows: the workspace names the sub-agent is limited to. */
  agentScope: string[] | null
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

/** A path's last segment: "src/lib/store.ts" → "store.ts". */
function baseName(path: string): string {
  const trimmed = path.replace(/\/+$/, '')
  return trimmed.slice(trimmed.lastIndexOf('/') + 1) || path
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1).replace(/\.0$/, '')} KB`
  return `${(n / 1024 / 1024).toFixed(1).replace(/\.0$/, '')} MB`
}

type Args = Record<string, unknown> | null
type Call = { name: string; args: unknown } | null

function argsOf(call: { args: unknown } | null): Args {
  return call?.args && typeof call.args === 'object' && !Array.isArray(call.args)
    ? (call.args as Record<string, unknown>)
    : null
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
    case 'list':
      // `proj:` / `@rumu:` (empty rest) is that workspace's root.
      return str('path') || (name === 'list' ? 'root' : '')
    case 'bash': {
      const command = str('command')
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean)
        .join(' ; ')
      const ws = str('workspace')
      if (command) return ws ? `${ws}: ${preview(command, 200)}` : preview(command, 200)
      return ws
    }
    case 'workspace_use':
      return str('name') || 'list'
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
      return argSummary(call)
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
      return str('path') ? baseName(str('path')) : ''
    case 'list': {
      const path = str('path').replace(/:$/, '')
      if (path && path !== '.' && path !== '/') return `${baseName(path)}/`
      return '/'
    }
    case 'bash':
      return preview(str('command'), 40)
    case 'code':
      return preview(str('code'), 40)
    case 'workspace_use':
      return str('name') || 'list'
    case 'search':
      return preview(str('query'), 40)
    case 'agent':
      return preview(str('task'), 40)
    default:
      return preview(argSummary(call), 40)
  }
}

/** `inherit` of an `agent` call, if it is an object. */
function inheritOf(args: Args): Args {
  const raw = args?.inherit
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Args) : null
}

/** True when an `agent` call asks for `context: "none"`. */
function agentNoContext(args: Args): boolean {
  return inheritOf(args)?.context === 'none'
}

/** The workspace names an `agent` call is scoped to, or null for all. */
function agentScope(args: Args): string[] | null {
  const raw = inheritOf(args)?.workspaces
  if (!Array.isArray(raw)) return null
  const names = raw.filter((w): w is string => typeof w === 'string')
  return names.length ? names : null
}

/** What the call produced, as a count or a word (empty when the call line says it). */
function resultShort(name: string, output: string): string {
  if (!output) return ''
  switch (name) {
    case 'workspace_use':
      return ''
    case 'list': {
      const n = nonEmptyLines(output).length
      return n ? t('tools.count.entries', { count: n }) : t('tools.result.empty')
    }
    case 'read':
      return t('tools.count.lines', { count: lineCount(output) })
    case 'write': {
      const m = /\((\d+) bytes?\)/.exec(output)
      return m ? fmtBytes(Number(m[1])) : ''
    }
    case 'edit': {
      // one replacement is the norm; only a multi-site edit is worth a word
      const m = /\((\d+) replacements?\)/.exec(output)
      return m && Number(m[1]) > 1 ? t('tools.count.edits', { count: Number(m[1]) }) : ''
    }
    case 'bash': {
      // a clean exit is the norm; say the exit code only when it isn't 0
      const m = /exit code:\s*(-?\d+)/.exec(output)
      if (m && m[1] !== '0') return t('tools.result.exit', { code: m[1] })
      const lines = nonEmptyLines(output.replace(/^.*exit code:\s*-?\d+\n?/, '')).length
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
  const name = row.call?.name ?? 'tool'
  // A card tool's target id, for its row chip: the current read/write/edit
  // whose path addresses `@rumu:/cards/<id>` (qualified always; `/cards/<id>`
  // and `cards/<id>` when `@rumu` is the branch default).
  const args = argsOf(row.call)
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
  let parsed = parsedCache.get(row.result)
  if (!parsed || parsed.locale !== locale) {
    parsed = {
      locale,
      callShort: callShort(name, row.call),
      callFull: callFull(name, row.call),
      // A blocked row is already explained by its stamp; an error keeps a
      // clipped first line so the row hints at what went wrong.
      result:
        blocked !== null
          ? ''
          : isError
            ? preview(nonEmptyLines(output)[0] ?? '', 40)
            : resultShort(name, output),
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
    callText: parsed.callShort,
    resultText: parsed.result,
    callFull: parsed.callFull,
    output: blocked ?? output,
    attachments,
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
    agentNoContext: name === 'agent' && agentNoContext(args),
    agentScope: name === 'agent' ? agentScope(args) : null,
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
// Folded phrases

/** Catalog key suffix for each call kind (see tools.json). */
const KIND_KEYS: Record<string, string> = {
  read: 'read',
  write: 'write',
  edit: 'edit',
  'read:card': 'readCard',
  'write:card': 'writeCard',
  'edit:card': 'editCard',
  list: 'list',
  bash: 'bash',
  code: 'code',
  agent: 'agent',
  search: 'search',
  workspace_use: 'workspaceUse',
}

/** A lone call's phrase names its target: "read store.ts", "ran cargo test". */
const TARGETED = new Set(['read', 'write', 'edit', 'list', 'bash', 'search', 'workspace_use'])

function kindKey(v: ToolRowView): string {
  if (v.toolCardId) return `${v.name}:card`
  return KIND_KEYS[v.name] ? v.name : 'other'
}

function catalogKind(key: string): string {
  return key === 'other' ? 'other' : (KIND_KEYS[key] ?? 'other')
}

/** A piece of a summary line; `mono` marks a literal (a file, a command). */
export interface PhrasePart {
  text: string
  mono?: boolean
}

/** Lowercase phrases for a run's tool calls, counted per kind in order of
 * first appearance; a lone call names its target instead. */
export function toolPhrases(views: ToolRowView[]): PhrasePart[][] {
  if (views.length === 1) {
    const v = views[0]!
    const key = kindKey(v)
    if (key === 'other') {
      return [[{ text: `${t('tools.verb.other')} ` }, { text: v.name, mono: true }]]
    }
    if (TARGETED.has(key) && v.callText) {
      const verb =
        key === 'workspace_use'
          ? v.callText === 'list'
            ? t('tools.verb.workspaceList')
            : t('tools.verb.workspaceUse')
          : t(`tools.verb.${catalogKind(key)}` as MessageKey)
      return [[{ text: `${verb} ` }, { text: v.callText, mono: true }]]
    }
  }
  const counts = new Map<string, number>()
  for (const v of views) {
    const key = kindKey(v)
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  return [...counts].map(([key, n]) => [
    { text: t(`tools.phrase.${catalogKind(key)}` as MessageKey, { count: n }) },
  ])
}
