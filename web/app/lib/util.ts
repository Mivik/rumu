import { SCRATCH_WORKSPACE, type Content } from './types'
import { locale, t } from '../i18n'

/** Compact relative time: "just now", "5m", "3h", "2d", or a date. */
export function relTime(ms: number, now = Date.now()): string {
  const diff = now - ms
  if (diff < 45_000) return t('common.time.justNow')
  const m = Math.round(diff / 60_000)
  if (m < 60) return t('common.time.minutesAgo', { n: m })
  const h = Math.round(m / 60)
  if (h < 24) return t('common.time.hoursAgo', { n: h })
  const d = Math.round(h / 24)
  if (d < 14) return t('common.time.daysAgo', { n: d })
  const at = new Date(ms)
  return at.toLocaleDateString(locale.value, {
    month: 'short',
    day: 'numeric',
    year: at.getFullYear() === new Date(now).getFullYear() ? undefined : 'numeric',
  })
}

/** The frozen text of a content payload (the model-facing projection). */
export function contentText(content: Content | undefined): string {
  return content?.text ?? ''
}

/** Short stable label for a node id. */
export function shortId(id: string): string {
  return id.slice(-6)
}

/** A workspace name as display copy: the session's scratch directory reads as
 * a plain label; registered workspace names stand as they are. */
export function workspaceLabel(name: string): string {
  return name === SCRATCH_WORKSPACE ? t('workspaces.scratch') : name
}

/** Copy text to the clipboard; returns whether the browser allowed it. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text)
    return true
  } catch {
    return false
  }
}

/** First line of a text, truncated — for previews. */
export function preview(text: string, max = 80): string {
  const line = text.split('\n').find((l) => l.trim()) ?? ''
  return line.length > max ? `${line.slice(0, max - 1)}…` : line
}

/** Tokens in `text`, roughly, as the kernel counts them: four ASCII chars,
 *  or one other char, each. */
export function textTokens(text: string): number {
  let ascii = 0
  let other = 0
  for (const c of text) {
    if (c.codePointAt(0)! < 128) ascii++
    else other++
  }
  return Math.ceil(ascii / 4) + other
}

/** Compact token count for meta lines: 1234 → '1.2k'. */
export function fmtTokens(n: number): string {
  if (n < 1000) return String(n)
  if (n < 10_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, '')}k`
  if (n < 1_000_000) return `${Math.round(n / 1000)}k`
  return `${(n / 1_000_000).toFixed(n < 10_000_000 ? 2 : 1).replace(/\.?0+$/, '')}M`
}

/** Compact byte size: 812 → '812 B', 587896 → '574 KB'. */
export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB']
  let v = n / 1024
  let i = 0
  for (; v >= 1024 && i < units.length - 1; i++) v /= 1024
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`
}

/** Compact thinking duration for the Thought disclosure: '<1s', '4s', '2m 05s'. */
export function fmtThought(ms: number): string {
  if (ms < 950) return '<1s'
  const s = Math.round(ms / 1000)
  if (s < 60) return `${Math.max(1, s)}s`
  const m = Math.floor(s / 60)
  return `${m}m ${String(s - m * 60).padStart(2, '0')}s`
}

/**
 * Precise-but-compact duration for usage stats. Precision follows magnitude so
 * the number stays around three significant digits, and units change only at
 * the points where the leading figure would otherwise get unwieldy:
 * '834ms' → '1.44s' → '39.9s' → '1m 05s' → '1h 05m'.
 *
 * The ms/s boundary is 1s, not an arbitrary 10s: 9.9s as '9900ms' reads worse
 * and jumps size; below a second, ms is where the precision actually lives.
 */
export function fmtDuration(ms: number): string {
  const total = Math.round(ms)
  if (total < 1000) return `${total}ms`
  if (total < 10_000) return `${String(Number((total / 1000).toFixed(2)))}s`
  if (total < 60_000) return `${String(Number((total / 1000).toFixed(1)))}s`
  const s = Math.round(total / 1000)
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
  const m = Math.round(s / 60)
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, '0')}m`
}

/** One-line argument summary for a tool call, preferring the obvious fields. */
export function argSummary(call: { name: string; args: unknown } | null): string {
  if (!call) return ''
  const a = call.args
  if (typeof a === 'string') return preview(a, 72)
  if (!a || typeof a !== 'object' || Array.isArray(a)) return ''
  const rec = a as Record<string, unknown>
  for (const k of [
    'command',
    'cmd',
    'task',
    'query',
    'path',
    'paths',
    'uri',
    'url',
    'pattern',
    'label',
    'lesson',
    'summary',
    'text',
    'name',
  ]) {
    const v = rec[k]
    if (typeof v === 'string' && v.trim()) return preview(v, 72)
    if (Array.isArray(v) && v.length && typeof v[0] === 'string') return preview(v.join(' '), 72)
  }
  const first = Object.values(rec)[0]
  if (typeof first === 'string' && first.trim()) return preview(first, 48)
  if (first !== undefined) return preview(JSON.stringify(first), 48)
  return ''
}

/** Drops case and the separators names and ids differ by (` -_./:`). */
const searchForm = (s: string): string => s.toLowerCase().replace(/[\s\-_./:]+/g, '')

/** Does a search find an item by its fields (name, id, …)? Each word of the
 *  query must turn up in one field, case and separators aside: `glm 5.3` and
 *  `glm5.3` find both `GLM-5.3 Highspeed` and `glm-5.3-highspeed`. */
export function searchMatches(query: string, ...fields: (string | null | undefined)[]): boolean {
  const words = query.split(/\s+/).map(searchForm).filter(Boolean)
  if (!words.length) return true
  const hay = fields.filter((f): f is string => !!f).map(searchForm)
  return words.every((w) => hay.some((h) => h.includes(w)))
}
