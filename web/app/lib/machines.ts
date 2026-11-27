// The other machines this server forwards to (`settings.yaml`'s
// `machines:`, `/m/<name>/…`), and which one the page shows.
//
// The page keeps one socket, to the serving machine, which forwards frames
// naming another machine to it (remote.rs). The page shows one machine at a
// time (`rumu.target`): its sessions open, its library, assistants,
// projects and settings show. Every machine's session list comes over the
// same socket, so the sidebar shows all of them as one list. A session's id
// is unique across machines, so the address needs no machine name: the
// machine whose list holds it opens it.
//
// A machine speaking another ws protocol (`ping`'s `protocol`, none for an
// older build) is not drawn from: its sessions stay out, and the machine
// menu opens its own page in a tab of its own.

import { computed, reactive, watch } from 'vue'
import { rumu } from './api'
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
  /** A switch is under way: the client's next ready call is the switch's,
   *  not a reconnect to resync (pushes.ts). */
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

/** Follow the machines the serving one lists (`settings.get`): read the
 *  new ones' lists. Nothing is set up while it lists none: a single machine
 *  keeps the page as it was. */
export function followMachines(list: { name: string; url: string }[], home: string): void {
  machines.home = home
  const wanted = list.length ? [{ name: '', url: location.origin }, ...list] : []
  const names = new Set(wanted.map((m) => m.name))
  const known = new Set(machines.all.map((m) => m.name))
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
  // Read after the list holds them: the reads write through it.
  if (rumu.open.value) for (const m of machines.all) if (!known.has(m.name)) void sync(m.name)
  if (machines.current && !names.has(machines.current)) void switchMachine('')
}

/** Read machine `name`: `ping` for its protocol (the first frame for it
 *  also opens the server's link to it), then its lists. */
async function sync(name: MachineName): Promise<void> {
  const m = machineState(name)
  if (!m) return
  try {
    // Only the pushes about no session in particular, unless it is shown:
    // then the store says which session it watches.
    if (name !== rumu.target) await rumu.request('session.watch', { session: null }, name)
    const pong = await rumu.request<{ protocol?: number }>('ping', {}, name)
    m.connected = true
    m.protocol = pong.protocol ?? 0
    if (!compatible(m)) return
    const [list] = await Promise.all([rumu.request<SessionInfo[]>('session.list', {}, name), readNames(name)])
    m.sessions = list
    m.listed = true
    for (const x of list) machines.of[x.id] = name
  } catch {
    // unreachable: its link says when it is back (`machine.status`)
  }
}

async function readNames(name: MachineName): Promise<void> {
  const m = live(name)
  if (!m) return
  const [projects, assistants] = await Promise.all([
    rumu.request<Project[]>('project.list', {}, name),
    rumu.request<Assistant[]>('assistant.list', {}, name),
  ])
  m.projects = projects
  m.assistants = assistants
}

function live(name: MachineName): MachineState | undefined {
  const m = machineState(name)
  return m && compatible(m) ? m : undefined
}

// Every machine's list pushes, the shown one's too: switching away from it
// leaves its list current.
rumu.onMachinePush('session.updated', (name, p) => {
  const m = live(name)
  if (!m) return
  const info = (p as { session: SessionInfo }).session
  const i = m.sessions.findIndex((x) => x.id === info.id)
  if (i >= 0) m.sessions[i] = info
  else m.sessions.push(info)
  machines.of[info.id] = name
})
rumu.onMachinePush('session.deleted', (name, p) => {
  const m = live(name)
  const id = (p as { session: { id: string } }).session.id
  if (m) m.sessions = m.sessions.filter((x) => x.id !== id)
})
rumu.onMachinePush('decks.updated', (name) => void readNames(name).catch(() => {}))
// A machine's link came up (read it again) or went down.
rumu.onMachinePush('machine.status', (name, p) => {
  const m = machineState(name)
  if (!m) return
  if ((p as { connected?: boolean }).connected) void sync(name)
  else {
    m.connected = false
    m.listed = false
  }
})
// The socket (re)opened: every machine is read again; the other machines'
// links open with their first frame.
watch(rumu.open, (up) => {
  if (up) for (const m of machines.all) void sync(m.name)
  else for (const m of machines.all) if (m.name) Object.assign(m, { connected: false, listed: false })
})

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
  return rumu.request<T>(method, params, '')
}

/** Every session the sidebar lists: the shown machine's own (`ours`, the
 *  store's, which opening and editing keep freshest) and the other
 *  machines' that speak this page's protocol. */
export function allSessions(ours: SessionInfo[]): MachineSession[] {
  const out: MachineSession[] = ours.map((s) => tag(s, machines.current))
  for (const m of machines.all) {
    if (m.name === machines.current || !compatible(m)) continue
    for (const s of m.sessions) out.push(tag(s, m.name))
  }
  return out
}

/** Tagged sessions, kept while the info is (a push replaces it): the list
 *  is merged again on every push, and a phone feels each copy of a few
 *  thousand sessions. */
const tagged = new WeakMap<SessionInfo, MachineSession>()

function tag(s: SessionInfo, machine: MachineName): MachineSession {
  let t = tagged.get(s)
  if (!t || t.machine !== machine) {
    t = { ...s, machine }
    tagged.set(s, t)
  }
  return t
}

/** Show machine `name`: requests and pushes are its from now on, and the
 *  store re-reads everything (the client's `start` callback) once it can be
 *  talked to. Resolves then. */
export function switchMachine(name: MachineName): Promise<void> {
  if (name === machines.current) return Promise.resolve()
  const m = machineState(name)
  if (name && (!m || !compatible(m))) return Promise.resolve()
  const left = machines.current
  machines.current = name
  machines.switching = true
  // Until its own lists come in, the machine's light ones stand in, so its
  // rows stay put in the sidebar and its groups keep their names.
  state.sessions = [...(m?.sessions ?? [])]
  state.projects = [...(m?.projects ?? [])]
  state.assistants = [...(m?.assistants ?? [])]
  state.decisions = []
  // The machine left sends only its list's pushes again.
  rumu.request('session.watch', { session: null }, left).catch(() => {})
  rumu.request('task.watch', { ids: [] }, left).catch(() => {})
  rumu.setTarget(name)
  return new Promise<void>((resolve) => {
    if (rumu.connected.value) return resolve()
    const stop = watch(rumu.connected, (up) => {
      if (up) {
        stop()
        resolve()
      }
    })
  })
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
