// Widgets: a ```widget fence in a reply is an HTML fragment drawn in place —
// a chart, a slider, a small calculator. The markdown renderer turns the
// fence into a `<rumu-widget data-key>` element and keeps its source here by
// key; the element, once in the page, fills itself with a frame running the
// fragment. The frame has an opaque origin, so it reaches nothing of rumu's;
// its network is the script CDNs alone. A small bridge in the page reports
// its height, takes the app's colors, and offers `rumu.prompt(text)`, which
// puts text in the composer for the user to send.

import { t } from '../i18n'
import { activeDraft } from './state/drafts'
import { requestComposerFocus } from './state/ui'

/** Whether a fence's info string names a widget. */
export function isWidgetFence(info: string | undefined): boolean {
  return info?.trim().split(/\s+/)[0]?.toLowerCase() === 'widget'
}

/** Sources by key: a key is the source's hash, so the same fence rendered
 *  again (a re-render, the streamed text landing) finds it. */
const sources = new Map<string, string>()
/** The last height each widget reported, so a redraw opens at it. */
const heights = new Map<string, number>()

function hash(s: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193)
  return (h >>> 0).toString(36) + s.length.toString(36)
}

/** Keep a widget's source; the key its element names it by. */
export function registerWidget(source: string): string {
  const key = hash(source)
  sources.set(key, source)
  return key
}

/** The app's colors a widget may use, as `var(--ink)` and so on. */
const COLORS = [
  '--canvas',
  '--surface',
  '--surface-2',
  '--line',
  '--line-soft',
  '--ink',
  '--ink-2',
  '--ink-3',
  '--brand',
  '--brand-soft',
  '--pine',
  '--tool',
  '--danger',
  '--memory',
  '--agent',
  '--warn',
]

interface Theme {
  scheme: string
  colors: Record<string, string>
}

function theme(): Theme {
  const style = getComputedStyle(document.documentElement)
  return {
    scheme: style.colorScheme || 'light',
    colors: Object.fromEntries(COLORS.map((c) => [c, style.getPropertyValue(c).trim()])),
  }
}

const CDNS = 'https://cdn.jsdelivr.net https://cdnjs.cloudflare.com https://unpkg.com'
const CSP =
  `default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' ${CDNS}; ` +
  `style-src 'unsafe-inline' ${CDNS}; font-src data: ${CDNS}; img-src data: blob:; media-src data: blob:`

/** Runs in the frame before the fragment. */
const BRIDGE = `(() => {
  const post = (m) => parent.postMessage(m, '*')
  window.rumu = { prompt: (text) => post({ rumu: 'prompt', text: String(text) }) }
  addEventListener('message', (e) => {
    const m = e.data
    if (e.source !== parent || !m || m.rumu !== 'theme') return
    const root = document.documentElement.style
    root.colorScheme = m.theme.scheme
    for (const [k, v] of Object.entries(m.theme.colors)) root.setProperty(k, v)
  })
  const root = document.documentElement
  new ResizeObserver(() => post({ rumu: 'height', height: Math.ceil(root.getBoundingClientRect().height) })).observe(root)
})()`

function page(source: string): string {
  const { scheme, colors } = theme()
  const vars = Object.entries(colors)
    .map(([k, v]) => `${k}:${v};`)
    .join('')
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    `<meta http-equiv="Content-Security-Policy" content="${CSP}">` +
    `<style>:root{color-scheme:${scheme};${vars}}` +
    'body{margin:0;color:var(--ink);accent-color:var(--brand);font:15px/1.6 system-ui,sans-serif}</style>' +
    `<script>${BRIDGE}</script></head><body>${source}</body></html>`
  )
}

/** The tallest a widget grows in the reply; past it, it scrolls. */
const MAX_HEIGHT = 720

class RumuWidget extends HTMLElement {
  connectedCallback(): void {
    const key = this.dataset.key ?? ''
    const source = sources.get(key)
    if (this.firstChild || !source) return
    const frame = document.createElement('iframe')
    frame.setAttribute('sandbox', 'allow-scripts')
    frame.setAttribute('referrerpolicy', 'no-referrer')
    frame.title = t('chat.widget.title')
    frame.style.height = `${Math.min(heights.get(key) ?? 160, MAX_HEIGHT)}px`
    frame.srcdoc = page(source)
    this.append(frame)
  }
}

function frameOf(source: MessageEventSource | null): HTMLIFrameElement | null {
  for (const frame of document.querySelectorAll<HTMLIFrameElement>('rumu-widget > iframe'))
    if (frame.contentWindow === source) return frame
  return null
}

function onMessage(e: MessageEvent): void {
  const m = e.data as { rumu?: string; height?: unknown; text?: unknown } | null
  if (!m || typeof m.rumu !== 'string') return
  const frame = frameOf(e.source)
  if (!frame) return
  if (m.rumu === 'height' && typeof m.height === 'number') {
    const key = (frame.parentElement as HTMLElement).dataset.key ?? ''
    heights.set(key, m.height)
    frame.style.height = `${Math.min(m.height, MAX_HEIGHT)}px`
  } else if (m.rumu === 'prompt' && typeof m.text === 'string' && m.text.trim()) {
    const draft = activeDraft.value
    draft.text = draft.text.trim() ? `${draft.text.trimEnd()}\n\n${m.text}` : m.text
    requestComposerFocus()
  }
}

/** Define the element and listen to the frames; once, client-side. */
export function installWidgets(): void {
  if (customElements.get('rumu-widget')) return
  customElements.define('rumu-widget', RumuWidget)
  window.addEventListener('message', onMessage)
  // The theme switched: hand the frames the new colors.
  new MutationObserver(() => {
    const now = theme()
    for (const frame of document.querySelectorAll<HTMLIFrameElement>('rumu-widget > iframe'))
      frame.contentWindow?.postMessage({ rumu: 'theme', theme: now }, '*')
  }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
}
