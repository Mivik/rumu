// WebSocket client for the rumu server.
//
// - JSON text frames: requests carry an id, responses resolve/reject a promise.
// - Pushes (frames without id) fan out to registered handlers.
// - Auto-reconnect with capped exponential backoff, cut short when the page
//   comes back to the foreground or the network comes back. Pending requests
//   are rejected on disconnect; callers (the store) re-fetch state on reconnect.

import { computed, reactive, ref, watch } from 'vue'
import type { WsMessage } from './types'
import { authorize } from './auth'

export type PushHandler = (params: Record<string, unknown>) => void

/** How long a foreground liveness ping may take before the socket counts as
 *  dead. Too short only costs a needless reconnect (and resync). */
const PROBE_TIMEOUT = 3000

/** A JSON-RPC error response; `code` is the server's error code (the web
 *  checks `-32010` for a stale card write, `-32011` for a main line held
 *  by background work, whose `data.tasks` names it). */
export class RpcError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly data?: unknown,
  ) {
    super(message)
    this.name = 'RpcError'
  }
}

/** The ws of the server serving the page. */
export function defaultWsUrl(): string {
  if (typeof location !== 'undefined') {
    const proto = location.protocol === 'https:' ? 'wss' : 'ws'
    return `${proto}://${location.host}/ws`
  }
  // Non-browser context (node smoke test): fall back to the dev server port.
  return 'ws://localhost:7878/ws'
}

/** A push from any machine: which one (`''`: this server), and its params. */
export type MachinePushHandler = (machine: string, params: Record<string, unknown>) => void

/** One socket, to the server serving the page. A frame naming a `machine`
 *  is for another machine the server reaches (remote.rs); what that
 *  machine sends back names it. Requests and pushes go to and come from the
 *  machine the page shows (`target`) unless said otherwise. */
export class RumuClient {
  /** The socket to this server is open. */
  readonly open = ref(false)
  /** Other machines whose link through this server is up. */
  readonly links: Record<string, boolean> = reactive({})
  private readonly shown = ref('')
  /** The machine the page shows can be talked to. */
  readonly connected = computed(
    () => this.open.value && (!this.shown.value || !!this.links[this.shown.value]),
  )

  private url: string
  private ws: WebSocket | null = null
  private nextId = 1
  private pending = new Map<
    number,
    { machine: string; resolve: (v: unknown) => void; reject: (e: Error) => void }
  >()
  private handlers = new Map<string, Set<PushHandler>>()
  private machineHandlers = new Map<string, Set<MachinePushHandler>>()
  private onConnected: (() => void) | null = null
  private attempts = 0
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null
  private stopped = false
  private generation = 0
  private probing = false

  constructor(url: string = defaultWsUrl()) {
    this.url = url
  }

  /** The machine the page shows (`''`: this server). */
  get target(): string {
    return this.shown.value
  }

  /** Where the shown machine's HTTP requests go: `''` for this server,
   *  `/m/<name>` for a machine it forwards to. */
  get base(): string {
    return this.shown.value ? `/m/${encodeURIComponent(this.shown.value)}` : ''
  }

  /** Show machine `name` from now on: `onReady` runs once it can be talked
   *  to, at once when it already can. */
  setTarget(name: string): void {
    if (name === this.shown.value) return
    const was = this.connected.value
    this.shown.value = name
    if (was && this.connected.value) this.onConnected?.()
  }

  /** Start connecting; `onReady` runs every time the shown machine can be
   *  talked to again. */
  start(onReady: () => void): void {
    this.onConnected = onReady
    watch(this.connected, (up) => up && this.onConnected?.(), { flush: 'sync' })
    this.reconnect()
    if (typeof document !== 'undefined') {
      // Back in front, or back online: the user is looking now, so don't sit
      // out a long backoff — retry at once and restart the backoff from 500ms.
      // A socket that still looks open may have died while the page was
      // suspended (mobile) without ever firing `close`: probe it.
      document.addEventListener('visibilitychange', () => {
        if (document.visibilityState !== 'visible') return
        this.retryNow()
        void this.probe()
      })
      window.addEventListener('online', () => {
        this.retryNow()
        void this.probe()
      })
    }
  }

  /** Cut a pending backoff short. No-op while connected, connecting, or
   *  stopped (e.g. waiting for a token). */
  retryNow(): void {
    if (this.stopped || this.reconnectTimer === null) return
    clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.attempts = 0
    void this.connect()
  }

  /** Ping an open socket; no answer within `PROBE_TIMEOUT` means it is dead:
   *  drop it and connect again right away. */
  private async probe(): Promise<void> {
    const ws = this.ws
    if (this.probing || !ws || ws.readyState !== WebSocket.OPEN) return
    this.probing = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const alive = await Promise.race([
      // Any answer will do, an error too (a server without `ping`).
      this.request('ping', {}, '').then(
        () => true,
        (e: unknown) => e instanceof RpcError,
      ),
      new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), PROBE_TIMEOUT)
      }),
    ])
    clearTimeout(timer)
    this.probing = false
    // Closed meanwhile: the `close` handler has taken over.
    if (alive || this.ws !== ws || this.stopped) return
    // Detach first: closing a dead socket may not fire `close` for a while.
    this.ws = null
    this.down()
    this.failPending(new Error('connection lost'))
    ws.close()
    this.attempts = 0
    void this.connect()
  }

  reconnect(): void {
    this.stop()
    this.stopped = false
    void this.connect()
  }

  stop(): void {
    this.stopped = true
    this.generation += 1
    this.down()
    if (this.reconnectTimer !== null) clearTimeout(this.reconnectTimer)
    this.reconnectTimer = null
    this.ws?.close()
    this.ws = null
    this.failPending(new Error('client stopped'))
  }

  private async connect(): Promise<void> {
    if (this.stopped) return
    const generation = this.generation
    try {
      if (typeof window !== 'undefined' && !(await authorize())) return
    } catch {
      if (generation === this.generation) this.scheduleReconnect()
      return
    }
    if (this.stopped || generation !== this.generation) return
    const ws = new WebSocket(this.url)
    this.ws = ws

    ws.addEventListener('open', () => {
      if (this.ws !== ws) return
      this.attempts = 0
      this.open.value = true
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
        this.down()
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

  /** The socket is gone, and the links through it with it. */
  private down(): void {
    this.open.value = false
    for (const name of Object.keys(this.links)) delete this.links[name]
  }

  private dispatch(msg: WsMessage & { machine?: string }): void {
    const from = msg.machine ?? ''
    if ('method' in msg && msg.method === 'machine.status') {
      const up = !!(msg.params as { connected?: boolean }).connected
      if (up) this.links[from] = true
      else {
        delete this.links[from]
        this.failPending(new Error(`machine ${from} unreachable`), from)
      }
      for (const h of this.machineHandlers.get(msg.method) ?? []) h(from, msg.params ?? {})
      return
    }
    if ('id' in msg) {
      const entry = this.pending.get(msg.id)
      if (!entry) return
      this.pending.delete(msg.id)
      if ('error' in msg) entry.reject(new RpcError(msg.error.message, msg.error.code, msg.error.data))
      else entry.resolve(msg.result)
    } else if ('method' in msg) {
      const run = (f: () => void) => {
        try {
          f()
        } catch (err) {
          console.error(`rumu: push handler for ${msg.method} failed`, err)
        }
      }
      for (const h of this.machineHandlers.get(msg.method) ?? []) run(() => h(from, msg.params ?? {}))
      if (from !== this.shown.value) return
      for (const h of this.handlers.get(msg.method) ?? []) run(() => h(msg.params ?? {}))
    }
  }

  /** Reject the requests in flight: all of them, or machine `machine`'s. */
  private failPending(err: Error, machine?: string): void {
    for (const [id, p] of this.pending) {
      if (machine !== undefined && p.machine !== machine) continue
      p.reject(err)
      this.pending.delete(id)
    }
  }

  /** Send a request to the shown machine, or to `machine` (`''`: this
   *  server); resolves with `result`, rejects on error or disconnect. */
  request<T = unknown>(
    method: string,
    params: Record<string, unknown> = {},
    machine = this.shown.value,
  ): Promise<T> {
    const ws = this.ws
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error('not connected'))
    }
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { machine, resolve: resolve as (v: unknown) => void, reject })
      const frame: Record<string, unknown> = { jsonrpc: '2.0', id, method, params }
      if (machine) frame.machine = machine
      ws.send(JSON.stringify(frame))
    })
  }

  /** Subscribe to a push from any machine; returns an unsubscribe function.
   *  `machine.status` (`{ connected }`) says a link came up or went down. */
  onMachinePush(method: string, handler: MachinePushHandler): () => void {
    let set = this.machineHandlers.get(method)
    if (!set) {
      set = new Set()
      this.machineHandlers.set(method, set)
    }
    set.add(handler)
    return () => {
      set?.delete(handler)
    }
  }

  /** Subscribe to a push from the shown machine; returns an unsubscribe
   *  function. */
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
