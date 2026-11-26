// Widgets: a ```widget fence in a reply is an HTML fragment drawn in place —
// a chart, a slider, a small calculator. The markdown renderer turns the
// fence into a `<rumu-widget data-key>` element and keeps its source here by
// key; the element, once in the page, fills itself with a frame running the
// fragment, and offers to save it as a file that opens as an artifact.
//
// Every model-written page runs here the same way, a widget or a `show`n
// file in the artifact panel: in an opaque-origin frame, so it reaches
// nothing of rumu's, its network the script CDNs alone. A small bridge in
// the page reports its height, takes the app's colors, and offers
// `rumu.prompt(text)`, which puts text in the composer for the user to send.

import { mdiApplicationOutline } from '@mdi/js'
import WIDGET_CSS from '../assets/css/widget.css?raw'
import { t } from '../i18n'
import { rumu as api } from './api'
import type { Artifact } from './types'
import { showArtifact } from './artifacts'
import { activeDraft } from './state/drafts'
import { state } from './state/state'
import { requestComposerFocus, toast } from './state/ui'

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
const PAGE_CSP =
  `default-src 'none'; script-src 'unsafe-inline' 'unsafe-eval' ${CDNS}; ` +
  `style-src 'unsafe-inline' ${CDNS}; font-src data: ${CDNS}; img-src data: blob:; media-src data: blob:`

/** Runs in the frame before the page's own markup: the height, the theme,
 *  `rumu.prompt`, a slider's fill (widget.css), and Chart.js in the app's
 *  colors — its defaults set the moment the library defines itself, and
 *  set again, charts redrawn, when the theme switches. */
const BRIDGE = `(() => {
  const post = (m) => parent.postMessage(m, '*')
  window.rumu = { prompt: (text) => post({ rumu: 'prompt', text: String(text) }) }
  const root = document.documentElement
  new ResizeObserver(() => post({ rumu: 'height', height: Math.ceil(root.getBoundingClientRect().height) })).observe(root)
  const fill = (r) => {
    const lo = r.min === '' ? 0 : +r.min, hi = r.max === '' ? 100 : +r.max
    r.style.setProperty('--fill', (hi > lo ? ((r.valueAsNumber - lo) / (hi - lo)) * 100 : 0) + '%')
  }
  addEventListener('input', (e) => e.target.type === 'range' && fill(e.target), true)
  addEventListener('DOMContentLoaded', () => document.querySelectorAll('input[type=range]').forEach(fill))

  const v = (n) => getComputedStyle(root).getPropertyValue(n).trim()
  const ROUND = ['pie', 'doughnut', 'polarArea']
  let chart, series = []
  /** Datasets this colored, so a new theme recolors them and no other. */
  const ours = new WeakSet()
  const paint = () => {
    const d = chart && chart.defaults
    // Only a widget's page has the app's colors (widget.css).
    if (!d || !v('--c1')) return false
    d.color = v('--ink-2')
    d.borderColor = v('--line')
    d.font.family = v('--font-sans')
    series = ['--c1', '--c2', '--c3', '--c4', '--c5', '--c6'].map(v)
    return true
  }
  Object.defineProperty(window, 'Chart', {
    configurable: true,
    get: () => chart,
    set: (c) => {
      chart = c
      if (!c || !c.register || !paint()) return
      if (c.defaults.plugins && c.defaults.plugins.colors) c.defaults.plugins.colors.enabled = false
      // Datasets without colors of their own take the series colors, one
      // per dataset (one per slice on a round chart).
      c.register({
        id: 'rumuColors',
        beforeLayout: (ch) => {
          const round = ROUND.includes(ch.config.type)
          ch.data.datasets.forEach((ds, i) => {
            if (!ours.has(ds) && (ds.backgroundColor !== undefined || ds.borderColor !== undefined)) return
            ours.add(ds)
            const each = round ? ch.data.labels.map((_, j) => series[j % series.length]) : series[i % series.length]
            ds.backgroundColor = each
            ds.borderColor = round ? v('--surface') : each
          })
        },
      })
    },
  })

  addEventListener('message', (e) => {
    const m = e.data
    if (e.source !== parent || !m || m.rumu !== 'theme') return
    root.style.colorScheme = m.theme.scheme
    for (const [k, val] of Object.entries(m.theme.colors)) root.style.setProperty(k, val)
    if (paint()) Object.values(chart.instances || {}).forEach((ch) => ch.update())
  })
})()`

/** `html` with `prelude` first thing after the doctype. The parser opens
 *  the head for it, so a CSP meta there applies before any of the page's
 *  own markup (a script ahead of `<head>` included). */
export function withPrelude(html: string, prelude: string): string {
  const at = /^\s*<!doctype[^>]*>/i.exec(html)?.[0].length ?? 0
  return html.slice(0, at) + prelude + html.slice(at)
}

/** A model-written page as a frame runs it: the CSP meta and the bridge first. */
export function sandboxedPage(html: string): string {
  return withPrelude(
    html,
    `<meta http-equiv="Content-Security-Policy" content="${PAGE_CSP}"><script>${BRIDGE}</script>`,
  )
}

/** The branch whose reply `el` is in: a side question's, in its card; a
 *  sub-agent's, in its panel. */
export function branchOf(el: Element): string | null {
  const side = el.closest<HTMLElement>('[data-branch]')?.dataset.branch
  if (side) return side
  return el.closest('[data-agent-panel]') ? state.agentView : state.mainBranch
}

/** Put `text` in the composer, after what is there, for the user to send. */
export function fillComposer(text: string): void {
  if (!text.trim()) return
  const draft = activeDraft.value
  draft.text = draft.text.trim() ? `${draft.text.trimEnd()}\n\n${text}` : text
  requestComposerFocus()
}

/** A widget as a page of its own, in today's colors and the base style
 *  (widget.css): what a frame runs it in, and the file it is saved as. */
function standalone(source: string): string {
  const { scheme, colors } = theme()
  const vars = Object.entries(colors)
    .map(([k, v]) => `${k}:${v};`)
    .join('')
  return (
    '<!doctype html><html><head><meta charset="utf-8">' +
    `<style>${WIDGET_CSS}\n:root{color-scheme:${scheme};${vars}}</style>` +
    `</head><body>${source}</body></html>`
  )
}

/** Save a widget as a file, recorded on the branch whose reply it is in
 *  (a sub-agent's, in its panel), and open it. */
async function save(source: string, from: Element): Promise<void> {
  const branch = branchOf(from)
  if (!branch) return
  try {
    const { artifact } = await api.request<{ artifact: Artifact }>('fs.save_page', {
      branch,
      page: standalone(source),
    })
    showArtifact(artifact)
  } catch (err) {
    toast('error', t('chat.widget.saveFailed'), (err as Error).message)
  }
}

/** The tallest a widget grows in the reply; past it, it scrolls. */
const MAX_HEIGHT = 720

const SAVE_ICON = `<svg width="14" height="14" viewBox="0 0 24 24" aria-hidden="true"><path fill="currentColor" d="${mdiApplicationOutline}"/></svg>`

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
    frame.srcdoc = sandboxedPage(standalone(source))
    const button = document.createElement('button')
    button.type = 'button'
    button.className = 'md-widget-save'
    button.title = button.ariaLabel = t('chat.widget.save')
    button.innerHTML = SAVE_ICON
    button.addEventListener('click', () => void save(source, this))
    this.append(frame, button)
  }
}

function frameOf(source: MessageEventSource | null): HTMLIFrameElement | null {
  for (const frame of document.querySelectorAll('iframe')) if (frame.contentWindow === source) return frame
  return null
}

function onMessage(e: MessageEvent): void {
  const m = e.data as { rumu?: string; height?: unknown; text?: unknown } | null
  if (!m || typeof m.rumu !== 'string') return
  const frame = frameOf(e.source)
  if (!frame) return
  const widget = frame.parentElement instanceof RumuWidget ? frame.parentElement : null
  // Only a widget follows its page's height: a panel's frame fills the panel.
  if (m.rumu === 'height' && typeof m.height === 'number' && widget) {
    heights.set(widget.dataset.key ?? '', m.height)
    frame.style.height = `${Math.min(m.height, MAX_HEIGHT)}px`
  } else if (m.rumu === 'prompt' && typeof m.text === 'string') {
    fillComposer(m.text)
  }
}

/** Define the element and listen to the frames; once, client-side. */
export function installWidgets(): void {
  if (customElements.get('rumu-widget')) return
  customElements.define('rumu-widget', RumuWidget)
  window.addEventListener('message', onMessage)
  // The theme switched: hand the widgets the new colors.
  new MutationObserver(() => {
    const now = theme()
    for (const frame of document.querySelectorAll<HTMLIFrameElement>('rumu-widget > iframe'))
      frame.contentWindow?.postMessage({ rumu: 'theme', theme: now }, '*')
  }).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] })
}
