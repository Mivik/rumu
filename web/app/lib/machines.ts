// The other machines this server forwards to (`settings.yaml`'s
// `machines:`, `/m/<name>/…`), and which one the page shows.
//
// The page talks to one machine at a time through the shared client
// (`rumu`): its sessions open, its library, assistants, projects and
// settings show. Every machine, the serving one too, has a light connection
// of its own for its session list, so the sidebar shows all of them as one
// list. A session's id is unique across machines, so the address needs no
// machine name: the machine whose list holds it opens it.
//
// A machine speaking another ws protocol (`ping`'s `protocol`, none for an
// older build) is not drawn from: its sessions stay out, and the machine
// menu opens its own page in a tab of its own.

import { computed, reactive, watch } from 'vue'
import { RumuClient, defaultWsUrl, rumu } from './api'
import { state } from './state/state'
import type { Assistant, Project, SessionId, SessionInfo } from './types'

/** The ws protocol this page speaks (`rumu_core::PROTOCOL`). */
export const PROTOCOL = 1

/** `''`: the machine serving the page. */
export type MachineName = string

export interface MachineState {
  name: MachineName
  /** Its own address (for its own page). */
  url: string
  connected: boolean
  /** The protocol it answered `ping` with; `null` until it did, `0` for
   *  none (a build from before there was one). */
  protocol: number | null
  /** Its session list has come in since it connected. */
  listed: boolean
  sessions: SessionInfo[]
  /** Its projects and assistants, for the names and marks on its rows. */
  projects: Project[]
  assistants: Assistant[]
}

export const machines = reactive({
  /** The machine the page shows (`''`: the one serving it). */
  current: '' as MachineName,
  /** The serving machine's own name (its host name), once known. */
  home: '',
  /** Every machine, the serving one (`''`) first. */
  all: [] as MachineState[],
  /** Which machine each session seen so far is on, kept across reloads, so
   *  a reload into `/s/<id>` connects to the right one at once. */
  of: load() as Record<SessionId, MachineName>,
  /** A switch is under way: the shared client's next connect is the
   *  switch's, not a reconnect to resync (pushes.ts). */
  switching: false,
})

const KEY = 'rumu.machineOf'

function load(): Record<SessionId, MachineName> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? '{}') as Record<SessionId, MachineName>
  } catch {
    return {}
  }
}

let saveTimer: ReturnType<typeof setTimeout> | undefined
watch(
  () => machines.of,
  () => {
    clearTimeout(saveTimer)
    saveTimer = setTimeout(() => {
      try {
        localStorage.setItem(KEY, JSON.stringify(machines.of))
      } catch {
        // only a shortcut
      }
    }, 500)
  },
  { deep: true },
)

/** Is any other machine set up: the sidebar then says where sessions are. */
export const multiMachine = computed(() => machines.all.length > 1)

/** A machine this page can show: it speaks the page's protocol. */
export function compatible(m: Pick<MachineState, 'protocol'>): boolean {
  return m.protocol === PROTOCOL
}

export function machineState(name: MachineName): MachineState | undefined {
  return machines.all.find((m) => m.name === name)
}

/** A machine's name to show: the serving one goes by its host name. */
export function machineLabel(name: MachineName): string {
  return name || machines.home || 'local'
}

/** The machine a session is on, as far as this page knows. */
export function machineOf(id: SessionId): MachineName | undefined {
  return machines.of[id]
}

const clients = new Map<MachineName, RumuClient>()

function base(name: MachineName): string {
  return name ? `/m/${encodeURIComponent(name)}` : ''
}

/** Follow the machines the serving one lists (`settings.get`): connect to
 *  new ones, drop gone ones. Nothing is set up while it lists none: a single
 *  machine keeps the page as it was. */
export function followMachines(list: { name: string; url: string }[], home: string): void {
  machines.home = home
  const wanted = list.length ? [{ name: '', url: location.origin }, ...list] : []
  const names = new Set(wanted.map((m) => m.name))
  for (const [name, c] of [...clients]) {
    if (!names.has(name)) {
      c.stop()
      clients.delete(name)
    }
  }
  machines.all = wanted.map((w) => {
    const m = machineState(w.name)
    if (m) {
      m.url = w.url
      return m
    }
    return {
      name: w.name,
      url: w.url,
      connected: false,
      protocol: w.name ? null : PROTOCOL,
      listed: false,
      sessions: [],
      projects: [],
      assistants: [],
    }
  })
  // Connected after the list holds them: the connections write through it.
  for (const m of machines.all) if (!clients.has(m.name)) clients.set(m.name, watchMachine(m.name))
  if (machines.current && !names.has(machines.current)) void switchMachine('')
}

/** A light connection: `ping` for the protocol, then the session list and
 *  its pushes (they reach every client, whatever it watches). */
function watchMachine(name: MachineName): RumuClient {
  const c = new RumuClient(defaultWsUrl(base(name)), false)
  const live = () => {
    const m = machineState(name)
    return m && compatible(m) ? m : undefined
  }
  c.onPush('session.updated', (p) => {
    const m = live()
    if (!m) return
    const info = (p as { session: SessionInfo }).session
    const i = m.sessions.findIndex((x) => x.id === info.id)
    if (i >= 0) m.sessions[i] = info
    else m.sessions.push(info)
    machines.of[info.id] = name
  })
  c.onPush('session.deleted', (p) => {
    const m = live()
    const id = (p as { session: { id: string } }).session.id
    if (m) m.sessions = m.sessions.filter((x) => x.id !== id)
  })
  const names = async () => {
    const m = live()
    if (!m) return
    const [projects, assistants] = await Promise.all([
      c.request<Project[]>('project.list'),
      c.request<Assistant[]>('assistant.list'),
    ])
    m.projects = projects
    m.assistants = assistants
  }
  c.onPush('decks.updated', () => void names().catch(() => {}))
  c.start(async () => {
    const m = machineState(name)
    if (!m) return
    m.connected = true
    try {
      // Only the pushes about no session in particular: the list's.
      await c.request('session.watch', { session: null })
      const pong = await c.request<{ protocol?: number }>('ping')
      m.protocol = pong.protocol ?? 0
      if (!compatible(m)) return
      const [list] = await Promise.all([c.request<SessionInfo[]>('session.list'), names()])
      m.sessions = list
      m.listed = true
      for (const x of list) machines.of[x.id] = name
    } catch {
      // the reconnect brings it back
    }
  })
  watch(c.connected, (up) => {
    const m = machineState(name)
    if (m && !up) {
      m.connected = false
      m.listed = false
    }
  })
  return c
}

/** A session in the merged list: which machine it is on. */
export type MachineSession = SessionInfo & { machine: MachineName }

/** The projects and assistants of machine `name`: the store's for the one
 *  shown. */
export function namesOf(name: MachineName): { projects: Project[]; assistants: Assistant[] } {
  if (name === machines.current) return state
  return machineState(name) ?? { projects: [], assistants: [] }
}

/** A request to the serving machine, whichever the page shows (the machine
 *  list is its). */
export function homeRequest<T>(method: string, params: Record<string, unknown> = {}): Promise<T> {
  return (machines.current ? clients.get('')! : rumu).request<T>(method, params)
}

/** Every session the sidebar lists: the shown machine's own (`ours`, the
 *  store's, which opening and editing keep freshest) and the other
 *  machines' that speak this page's protocol. */
export function allSessions(ours: SessionInfo[]): MachineSession[] {
  const out: MachineSession[] = ours.map((s) => ({ ...s, machine: machines.current }))
  for (const m of machines.all) {
    if (m.name === machines.current || !compatible(m)) continue
    for (const s of m.sessions) out.push({ ...s, machine: m.name })
  }
  return out
}

/** Show machine `name`: the shared client reconnects there, and the store
 *  re-reads everything as it connects (its `start` callback). Resolves once
 *  connected. */
export function switchMachine(name: MachineName): Promise<void> {
  if (name === machines.current) return Promise.resolve()
  const m = machineState(name)
  if (name && (!m || !compatible(m))) return Promise.resolve()
  machines.current = name
  machines.switching = true
  // Until its own lists come in, the machine's light ones stand in, so its
  // rows stay put in the sidebar.
  state.sessions = [...(machineState(name)?.sessions ?? [])]
  state.decisions = []
  rumu.setBase(base(name))
  const ready = new Promise<void>((resolve) => {
    const stop = watch(rumu.connected, (up) => {
      if (up) {
        stop()
        resolve()
      }
    })
  })
  rumu.reconnect()
  return ready
}

/** Which machine holds `id`: the one remembered, else the first whose list
 *  has it, waiting (up to `ms`) for lists still on their way. `null`: none
 *  of them. */
export function locate(id: SessionId, ms = 6000): Promise<MachineName | null> {
  const find = (): MachineName | null | undefined => {
    const known = machines.of[id]
    if (known !== undefined && machineState(known)) return known
    for (const m of machines.all) if (m.sessions.some((s) => s.id === id)) return m.name
    // every machine that can answer has answered: nobody has it
    if (machines.all.every((m) => m.listed || (m.protocol !== null && !compatible(m)))) return null
    return undefined
  }
  const now = find()
  if (now !== undefined) return Promise.resolve(now)
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      stop()
      resolve(null)
    }, ms)
    const stop = watch(
      () => find(),
      (v) => {
        if (v === undefined) return
        clearTimeout(timer)
        stop()
        resolve(v)
      },
    )
  })
}
