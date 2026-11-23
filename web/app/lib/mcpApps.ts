// MCP Apps (the `io.modelcontextprotocol/ui` extension): a tool result whose
// tool names a `ui://` page carries that page and the call as blobs (the
// `mcp.app` payload, recorded by the server when the tool ran). Its view is
// the page in an opaque-origin frame, under the CSP its server declared,
// talking JSON-RPC over postMessage: the view gets the call's arguments and
// result, calls tools and reads resources of its own server (`mcp.app_call`,
// `mcp.app_read`), puts text in the composer (`ui/message`, the user sends
// it), tells the model something for its next call (`mcp.app_context`), and
// may go fullscreen.

import { rumu as api } from './api'
import { artifactText } from './artifacts'
import type { Content } from './types'
import type { McpApp } from './generated/McpApp'
import type { McpAppCsp } from './generated/McpAppCsp'
import { fillComposer, withPrelude } from './widgets'

export type { McpApp }

/** The `mcp.app` payload of a tool result, if it has a view. */
export function appOf(content: Content | null | undefined): McpApp | null {
  const d = content?.data
  return d && d.tag === 'mcp.app' && d.v === 1 ? (d.data as McpApp) : null
}

/** The tallest a view grows inline; past it, it scrolls. */
export const MAX_HEIGHT = 720

/** The last height each view (by result node) reported, so a redraw opens at it. */
export const viewHeights = new Map<string, number>()

/** The CSP of a view: its declared origins and nothing more. No `'self'`:
 *  a srcdoc frame's self is rumu's origin. */
function policy(csp: McpAppCsp): string {
  const list = (xs: string[], none = "'none'") => (xs.length ? xs.join(' ') : none)
  const res = csp.resource.join(' ')
  return [
    "default-src 'none'",
    `script-src 'unsafe-inline' ${res}`,
    `style-src 'unsafe-inline' ${res}`,
    `img-src data: blob: ${res}`,
    `font-src data: ${res}`,
    `media-src data: blob: ${res}`,
    `connect-src ${list(csp.connect)}`,
    `frame-src ${list(csp.frame)}`,
    "object-src 'none'",
    `base-uri ${list(csp.base_uri)}`,
  ].join('; ')
}

/** The page a view's frame runs: the server's, its CSP first. */
export async function appPage(app: McpApp): Promise<string> {
  const html = await artifactText(app.html)
  const csp = policy(app.csp).replaceAll('"', '')
  return withPrelude(html, `<meta http-equiv="Content-Security-Policy" content="${csp}">`)
}

/** The call a view shows: its arguments and the server's result. */
export async function appCall(app: McpApp): Promise<{ arguments: unknown; result: unknown }> {
  return JSON.parse(await artifactText(app.call))
}

/** The app's colors as the standard variables a view may use. */
const VARIABLES: Record<string, string> = {
  '--color-background-primary': '--surface',
  '--color-background-secondary': '--surface-2',
  '--color-background-tertiary': '--canvas',
  '--color-background-inverse': '--ink',
  '--color-background-info': '--brand-soft',
  '--color-text-primary': '--ink',
  '--color-text-secondary': '--ink-2',
  '--color-text-tertiary': '--ink-3',
  '--color-text-inverse': '--surface',
  '--color-text-info': '--brand',
  '--color-text-danger': '--danger',
  '--color-text-success': '--pine',
  '--color-text-warning': '--warn',
  '--color-border-primary': '--line',
  '--color-border-secondary': '--line-soft',
  '--color-border-tertiary': '--line-soft',
  '--color-border-info': '--brand',
  '--color-border-danger': '--danger',
  '--color-border-success': '--pine',
  '--color-border-warning': '--warn',
  '--color-ring-primary': '--brand',
  '--font-sans': '--font-sans',
  '--font-mono': '--font-mono',
}

/** The theme part of a view's host context, as the app looks now. */
export function themeContext(): Record<string, unknown> {
  const style = getComputedStyle(document.documentElement)
  const variables: Record<string, string> = {
    '--border-radius-sm': '6px',
    '--border-radius-md': '8px',
    '--border-radius-lg': '12px',
  }
  for (const [k, ours] of Object.entries(VARIABLES)) {
    const v = style.getPropertyValue(ours).trim()
    if (v) variables[k] = v
  }
  return { theme: style.colorScheme === 'dark' ? 'dark' : 'light', styles: { variables } }
}

type Id = string | number
interface Message {
  jsonrpc?: string
  id?: Id
  method?: string
  params?: Record<string, unknown>
  result?: unknown
  error?: unknown
}

/** What a view's bridge needs of its place in the page. */
export interface ViewHost {
  frame: HTMLIFrameElement
  /** The element fullscreen shows. */
  box: HTMLElement
  app: McpApp
  call: { arguments: unknown; result: unknown }
  /** The result node: the view's name for its model context. */
  node: string
  /** The branch it is on, when it is known. */
  branch: () => string | null
  /** The view reported its height. */
  onHeight: (height: number) => void
}

/** Text blocks' text, joined. */
function textOf(content: unknown): string {
  const blocks = Array.isArray(content) ? content : [content]
  return blocks
    .map((b) =>
      b && typeof b === 'object' && (b as { type?: unknown }).type === 'text'
        ? (b as { text?: unknown }).text
        : null,
    )
    .filter((t): t is string => typeof t === 'string')
    .join('\n\n')
}

/** One view's end of the JSON-RPC conversation. */
export class AppBridge {
  private initialized = false
  private nextId = 1
  private fullscreen = false
  private readonly listener = (e: MessageEvent) => void this.receive(e)
  private readonly onFullscreen = () => this.fullscreenChanged()

  constructor(private readonly host: ViewHost) {
    window.addEventListener('message', this.listener)
    document.addEventListener('fullscreenchange', this.onFullscreen)
  }

  /** The view goes away: tell it, without waiting. */
  close(): void {
    if (this.initialized)
      this.post({
        jsonrpc: '2.0',
        id: `teardown-${this.nextId++}`,
        method: 'ui/resource-teardown',
        params: { reason: 'unmounted' },
      })
    window.removeEventListener('message', this.listener)
    document.removeEventListener('fullscreenchange', this.onFullscreen)
  }

  /** The app's theme changed. */
  themeChanged(): void {
    if (this.initialized) this.notify('ui/notifications/host-context-changed', themeContext())
  }

  private post(m: Message): void {
    // As plain data: the payload may be reactive state, which does not clone.
    this.host.frame.contentWindow?.postMessage(JSON.parse(JSON.stringify(m)), '*')
  }

  private notify(method: string, params: Record<string, unknown>): void {
    this.post({ jsonrpc: '2.0', method, params })
  }

  private dimensions(): Record<string, number> {
    return this.fullscreen
      ? { width: window.innerWidth, height: window.innerHeight }
      : { width: this.host.box.clientWidth, maxHeight: MAX_HEIGHT }
  }

  private async receive(e: MessageEvent): Promise<void> {
    if (e.source !== this.host.frame.contentWindow) return
    const m = e.data as Message | null
    if (!m || m.jsonrpc !== '2.0' || typeof m.method !== 'string') return
    const params = m.params ?? {}
    if (m.id === undefined) return this.notification(m.method, params)
    try {
      this.post({ jsonrpc: '2.0', id: m.id, result: await this.request(m.method, params) })
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      const code = err instanceof Unknown ? -32601 : -32000
      this.post({ jsonrpc: '2.0', id: m.id, error: { code, message } })
    }
  }

  private notification(method: string, params: Record<string, unknown>): void {
    if (method === 'ui/notifications/initialized') {
      if (this.initialized) return
      this.initialized = true
      this.notify('ui/notifications/tool-input', { arguments: this.host.call.arguments ?? {} })
      this.notify('ui/notifications/tool-result', this.host.call.result as Record<string, unknown>)
    } else if (method === 'ui/notifications/size-changed') {
      if (typeof params.height === 'number' && !this.fullscreen) this.host.onHeight(params.height)
    } else if (method === 'notifications/message') {
      console.debug(`[mcp app ${this.host.app.server}]`, params.data)
    }
  }

  private async request(method: string, params: Record<string, unknown>): Promise<unknown> {
    const { app } = this.host
    switch (method) {
      case 'ui/initialize':
        return {
          protocolVersion: typeof params.protocolVersion === 'string' ? params.protocolVersion : '2026-01-26',
          hostInfo: { name: 'rumu', version: '1' },
          hostCapabilities: {
            openLinks: {},
            message: { text: {} },
            updateModelContext: { text: {}, structuredContent: {} },
            serverTools: {},
            serverResources: {},
            logging: {},
            sandbox: {
              csp: {
                connectDomains: app.csp.connect,
                resourceDomains: app.csp.resource,
                frameDomains: app.csp.frame,
                baseUriDomains: app.csp.base_uri,
              },
            },
          },
          hostContext: {
            ...themeContext(),
            displayMode: this.fullscreen ? 'fullscreen' : 'inline',
            availableDisplayModes: document.fullscreenEnabled ? ['inline', 'fullscreen'] : ['inline'],
            containerDimensions: this.dimensions(),
            locale: navigator.language,
            timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
            userAgent: 'rumu',
            platform: 'web',
            deviceCapabilities: {
              touch: matchMedia('(pointer: coarse)').matches,
              hover: matchMedia('(hover: hover)').matches,
            },
          },
        }
      case 'ping':
        return {}
      case 'tools/call':
        return api.request('mcp.app_call', {
          branch: this.branch(),
          server: app.server,
          tool: params.name,
          arguments: params.arguments ?? {},
        })
      case 'resources/read':
        return api.request('mcp.app_read', { branch: this.branch(), server: app.server, uri: params.uri })
      case 'ui/open-link': {
        const url = new URL(String(params.url))
        if (url.protocol !== 'https:' && url.protocol !== 'http:') throw new Error('Only http(s) links open')
        window.open(url, '_blank', 'noopener,noreferrer')
        return {}
      }
      case 'ui/message':
        fillComposer(textOf(params.content))
        return {}
      case 'ui/update-model-context':
        await api.request('mcp.app_context', {
          branch: this.branch(),
          view: this.host.node,
          server: app.server,
          tool: app.tool,
          content: params.content ?? null,
          structured: params.structuredContent ?? null,
        })
        return {}
      case 'ui/request-display-mode':
        return { mode: await this.displayMode(params.mode) }
      default:
        throw new Unknown(`Unknown method ${method}`)
    }
  }

  private branch(): string {
    const branch = this.host.branch()
    if (!branch) throw new Error('No branch')
    return branch
  }

  /** Switch to `mode` when the browser can; the mode the view is in after. */
  private async displayMode(mode: unknown): Promise<string> {
    try {
      if (mode === 'fullscreen' && !this.fullscreen && document.fullscreenEnabled)
        await this.host.box.requestFullscreen()
      else if (mode === 'inline' && this.fullscreen) await document.exitFullscreen()
    } catch (err) {
      // Refused (no user gesture, say): the mode stays.
      console.warn('[mcp app] display mode', mode, err)
    }
    return document.fullscreenElement === this.host.box ? 'fullscreen' : 'inline'
  }

  private fullscreenChanged(): void {
    const now = document.fullscreenElement === this.host.box
    if (now === this.fullscreen) return
    this.fullscreen = now
    if (this.initialized)
      this.notify('ui/notifications/host-context-changed', {
        displayMode: now ? 'fullscreen' : 'inline',
        containerDimensions: this.dimensions(),
      })
  }
}

/** A method the bridge does not serve. */
class Unknown extends Error {}
