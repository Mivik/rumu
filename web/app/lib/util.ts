import type { Content } from './types'
import { t } from '../i18n'

/** Compact relative time: "just now", "5m", "3h", "2d", or a date. */
export function relTime(ms: number): string {
  const diff = Date.now() - ms
  if (diff < 45_000) return t('common.time.justNow')
  const m = Math.round(diff / 60_000)
  if (m < 60) return t('common.time.minutesAgo', { n: m })
  const h = Math.round(m / 60)
  if (h < 24) return t('common.time.hoursAgo', { n: h })
  const d = Math.round(h / 24)
  if (d < 14) return t('common.time.daysAgo', { n: d })
  return new Date(ms).toLocaleDateString()
}

/** The frozen text of a content payload (the model-facing projection). */
export function contentText(content: Content | undefined): string {
  return content?.text ?? ''
}

/** Short stable label for a node id. */
export function shortId(id: string): string {
  return id.slice(-6)
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

/** Compact token count for meta lines: 1234 → '1.2k'. */
export function fmtTokens(n: number): string {
  if (n < 1000) return String(n)
  if (n < 10_000) return `${(n / 1000).toFixed(1).replace(/\.0$/, '')}k`
  return `${Math.round(n / 1000)}k`
}

/** Compact thinking duration for the Thought disclosure: '<1s', '4s', '2m 05s'. */
export function fmtThought(ms: number): string {
  if (ms < 950) return '<1s'
  const s = Math.round(ms / 1000)
  if (s < 60) return `${Math.max(1, s)}s`
  const m = Math.floor(s / 60)
  return `${m}m ${String(s - m * 60).padStart(2, '0')}s`
}

/** Precise-but-compact duration for usage stats: '850ms', '1.2s', '1m 05s'. */
export function fmtDuration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(1).replace(/\.0$/, '')}s`
  const s = Math.round(ms / 1000)
  const m = Math.floor(s / 60)
  return `${m}m ${String(s - m * 60).padStart(2, '0')}s`
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
