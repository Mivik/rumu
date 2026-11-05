// WebSocket client for the rumu server.
//
// - JSON text frames: requests carry an id, responses resolve/reject a promise.
// - Pushes (frames without id) fan out to registered handlers.
// - Auto-reconnect with capped exponential backoff. Pending requests are
//   rejected on disconnect; callers (the store) re-fetch state on reconnect.

import { ref, type Ref } from 'vue'
import type { WsMessage } from './types'

export type PushHandler = (params: Record<string, unknown>) => void

/** A JSON-RPC error response; `code` is the server's error code (the web
 *  checks `-32010` for a stale card write). */
export class RpcError extends Error {
  constructor(
    message: string,
    readonly code: number,
  ) {
    super(message)
    this.name = 'RpcError'
  }
}

export function defaultWsUrl(): string {
  if (typeof location !== 'undefined') {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    return `${proto}://${location.host}/ws`
  }
  // Non-browser context (node smoke test): fall back to the dev server port.
  return 'ws://localhost:7878/ws'
}

export class RumuClient {
  readonly connected: Ref<boolean> = ref(false)

  private url: string
  private ws: WebSocket | null = null
  private nextId = 1
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>()
  private handlers = new Map<string, Set<PushHandler>>()
  private onConnected: (() => void) | null = null
  private attempts = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private stopped = false

  constructor(url: string = defaultWsUrl()) {
    this.url = url
  }

  /** Start connecting; `onReady` runs after every (re)connection. */
  start(onReady: () => void): void {
    this.onConnected = onReady
    this.stopped = false
    this.connect()
  }

  stop(): void {
    this.stopped = true
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.ws?.close()
    this.ws = null
    this.failPending(new Error('client stopped'))
  }

  private connect(): void {
    if (this.stopped) return
    const ws = new WebSocket(this.url)
    this.ws = ws

    ws.addEventListener('open', () => {
      this.attempts = 0
      this.connected.value = true
      this.onConnected?.()
    })

    ws.addEventListener('message', (ev: MessageEvent) => {
      let msg: WsMessage
      try {
        msg = JSON.parse(typeof ev.data === 'string' ? ev.data : '') as WsMessage
      } catch {
        console.warn('rumu: malformed ws frame', ev.data)
        return
      }
      this.dispatch(msg)
    })

    ws.addEventListener('close', () => {
      if (this.ws === ws) {
        this.ws = null
        this.connected.value = false
        this.failPending(new Error('connection closed'))
        this.scheduleReconnect()
      }
    })
  }

  private scheduleReconnect(): void {
    if (this.stopped) return
    const delay = Math.min(500 * 2 ** this.attempts, 10_000)
    this.attempts += 1
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null
      this.connect()
    }, delay)
  }

  private dispatch(msg: WsMessage): void {
    if ('id' in msg) {
      const entry = this.pending.get(msg.id)
      if (!entry) return
      this.pending.delete(msg.id)
      if ('error' in msg) entry.reject(new RpcError(msg.error.message, msg.error.code))
      else entry.resolve(msg.result)
    } else if ('method' in msg) {
      const set = this.handlers.get(msg.method)
      if (!set) return
      for (const h of set) {
        try {
          h(msg.params ?? {})
        } catch (err) {
          console.error(`rumu: push handler for ${msg.method} failed`, err)
        }
      }
    }
  }

  private failPending(err: Error): void {
    for (const { reject } of this.pending.values()) reject(err)
    this.pending.clear()
  }

  /** Send a request; resolves with `result`, rejects on error or disconnect. */
  request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    const ws = this.ws
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error('not connected'))
    }
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject })
      ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
    })
  }

  /** Subscribe to a push; returns an unsubscribe function. */
  onPush(method: string, handler: PushHandler): () => void {
    let set = this.handlers.get(method)
    if (!set) {
      set = new Set()
      this.handlers.set(method, set)
    }
    set.add(handler)
    return () => {
      set?.delete(handler)
    }
  }
}

/** Shared app-wide client. */
export const rumu = new RumuClient()
