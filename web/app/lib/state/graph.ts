// The branch view's data. `session.graph` gives the main line's turns across
// every version and where each sub-agent, side question and main-line task
// hangs from them; it is fetched while the view is open and again shortly
// after a push that changes it. `graphModel` lays it out: one column of
// messages — the current version, or the one picked at each fork — with the
// sub-agents as trees off them and the tasks inside their owners' cards.
// Whether something runs comes from the live state (`state.busy`, running
// tasks), never from a branch's recorded status.

import { reactive } from 'vue'
import { rumu } from '../api'
import type {
  Branch,
  BranchId,
  BranchUpdatedPush,
  GraphBranch,
  GraphTurn,
  NodeId,
  SessionGraph,
  TaskInfo,
  TaskInfoPush,
  TurnPush,
} from '../types'
import { pathNodes } from './derived'
import { showView } from './sessions'
import { state } from './state'
import { goToTurn } from './tree'
import { focusTarget } from './ui'

/** The graph of the open session, once fetched. */
export const graph = reactive({
  session: null as string | null,
  data: null as SessionGraph | null,
  failed: false,
})

let seq = 0

/** Fetch the open session's graph. */
export async function loadGraph(): Promise<void> {
  const session = state.sessionId
  if (!session) return
  const mine = ++seq
  try {
    const data = await rumu.request<SessionGraph>('session.graph', { session })
    if (mine !== seq) return
    graph.session = session
    graph.data = data
    graph.failed = false
  } catch {
    if (mine === seq) graph.failed = true
  }
}

/** How long after a push the graph is fetched again: pushes come in bursts. */
const REFRESH_MS = 500

/** Fetch the graph and keep it fresh until the returned stop is called: a
 *  new branch or a moved head, a task started (a main-line one needs its
 *  turn), and any turn's end (the totals move). */
export function watchGraph(): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined
  const soon = (): void => {
    timer ??= setTimeout(() => {
      timer = undefined
      void loadGraph()
    }, REFRESH_MS)
  }
  const offs = [
    rumu.onPush('branch.updated', (p) => {
      if ((p as unknown as BranchUpdatedPush).branch.session === state.sessionId) soon()
    }),
    rumu.onPush('task.started', (p) => {
      if ((p as unknown as TaskInfoPush).task.session === state.sessionId) soon()
    }),
    rumu.onPush('turn.done', (p) => {
      if (state.branches[(p as unknown as TurnPush).branch]) soon()
    }),
  ]
  void loadGraph()
  return () => {
    for (const off of offs) off()
    clearTimeout(timer)
  }
}

/** Show the main line's message `user`, turn `n` of it (1-based), in the
 *  conversation: its stretch of the line loads first when it is not here. */
export async function showMessage(user: NodeId, n: number): Promise<void> {
  const session = state.sessionId
  if (!session) return
  if (!pathNodes.value.some((node) => node.id === user)) await goToTurn(n)
  if (state.sessionId !== session) return
  focusTarget.value = { session, node: user }
  showView('chat')
}

/** A sub-agent or side question, with what it started. */
export interface GraphNode {
  id: BranchId
  branch: Branch
  /** Its background task, for one started in the background. */
  task: TaskInfo | null
  sum: GraphBranch | null
  /** Sub-agents and side questions it started, oldest first. */
  kids: GraphNode[]
  /** Its own tasks, running ones first. */
  tasks: TaskInfo[]
  /** Its loop runs, or its background run has not ended. */
  running: boolean
  /** Its tasks still running. */
  runningTasks: number
  /** Hangs from a version of the main line other than the current one:
   *  its replies stay on its own branch. */
  detached: boolean
  /** Branches below it. */
  total: number
  /** Its colour, 1–4 (`--g1`…, BranchGraph). */
  accent: number
}

/** A message with several versions, as its switcher shows them. */
export interface GraphVersion {
  /** Where the pick is kept: the turn before (empty for the first). */
  fork: string
  ids: NodeId[]
  /** The version shown and the current one, as indexes into `ids`. */
  shown: number
  current: number
  regen: boolean
  /** Things still running in the versions not shown. */
  others: number
}

export interface GraphTurnItem {
  kind: 'turn'
  id: NodeId
  turn: GraphTurn
  kids: GraphNode[]
  tasks: TaskInfo[]
  ver: GraphVersion | null
  /** A version other than the current one. */
  off: boolean
}

/** Messages in a row that started nothing, folded into one line. */
export interface GraphQuietItem {
  kind: 'quiet'
  id: string
  turns: GraphTurnItem[]
  off: boolean
}

export type GraphItem = GraphTurnItem | GraphQuietItem

export interface GraphInput {
  graph: SessionGraph
  main: BranchId
  branches: Record<BranchId, Branch>
  tasks: TaskInfo[]
  busy: Record<BranchId, boolean>
}

export interface GraphModel {
  items: GraphItem[]
  nodes: Record<BranchId, GraphNode>
  /** The turn each top-level node hangs from. */
  turnOf: Record<BranchId, NodeId>
  /** Each turn's parent turn. */
  parentOf: Record<NodeId, NodeId | null>
  /** The main line has no message yet. */
  empty: boolean
}

interface TurnSlot {
  turn: GraphTurn
  kids: GraphNode[]
  tasks: TaskInfo[]
  next: TurnSlot[]
}

const byRunningThenAge = (a: TaskInfo, b: TaskInfo): number =>
  Number(b.status === 'running') - Number(a.status === 'running') || a.started_at - b.started_at

/** Lay the graph out (see the module comment). `pick` holds the version
 *  picked at each fork, by `GraphVersion.fork`; the current one elsewhere. */
export function graphModel(input: GraphInput, pick: Record<string, NodeId>): GraphModel {
  const { graph, main, branches, tasks, busy } = input
  const slots: Record<NodeId, TurnSlot> = {}
  for (const turn of graph.turns) slots[turn.id] = { turn, kids: [], tasks: [], next: [] }
  const roots: TurnSlot[] = []
  for (const s of Object.values(slots)) (s.turn.parent ? slots[s.turn.parent]?.next : roots)?.push(s)
  // what the graph has no anchor for yet (it came after the last fetch):
  // the line's latest turn that began before it
  const line = graph.turns.filter((t) => t.line)
  const turnAt = (at: number): TurnSlot | null => {
    let found = line[0]
    for (const t of line) if (t.at <= at) found = t
    return found ? slots[found.id]! : null
  }
  const anchored = (id: string, at: number): TurnSlot | null => slots[graph.anchors[id] ?? ''] ?? turnAt(at)

  // background agents' tasks, by the branch they run
  const agentTask: Record<BranchId, TaskInfo> = {}
  for (const t of tasks) {
    if (t.kind.type !== 'agent') continue
    const child =
      t.kind.child_branch ??
      Object.values(branches).find(
        (b) => b.kind.type === 'agent' && b.kind.call_id === t.call_id && b.kind.parent_branch === t.branch,
      )?.id
    if (child) agentTask[child] = t
  }

  const nodes: Record<BranchId, GraphNode> = {}
  const list = Object.values(branches)
    .filter((b) => b.id !== main && b.kind.type !== 'main')
    .sort((a, b) => a.created_at - b.created_at)
  list.forEach((b, i) => {
    const task = agentTask[b.id] ?? null
    nodes[b.id] = {
      id: b.id,
      branch: b,
      task,
      sum: graph.branches[b.id] ?? null,
      kids: [],
      tasks: [],
      running: !!busy[b.id] || task?.status === 'running',
      runningTasks: 0,
      detached: false,
      total: 0,
      accent: (i % 4) + 1,
    }
  })
  const turnOf: Record<BranchId, NodeId> = {}
  const tops: GraphNode[] = []
  for (const b of list) {
    const n = nodes[b.id]!
    const parent = b.kind.type === 'main' ? undefined : nodes[b.kind.parent_branch]
    if (parent) {
      parent.kids.push(n)
      continue
    }
    const slot = anchored(b.id, b.created_at)
    if (!slot) continue
    slot.kids.push(n)
    turnOf[b.id] = slot.turn.id
    tops.push(n)
  }
  const shownAsCards = new Set(Object.values(agentTask).map((t) => t.id))
  for (const t of tasks) {
    if (shownAsCards.has(t.id)) continue
    if (t.branch === main) anchored(t.id, t.started_at)?.tasks.push(t)
    else nodes[t.branch]?.tasks.push(t)
  }

  // what each node adds up to; a node off the current version is detached
  // with everything below it
  const live = (n: GraphNode): number =>
    Number(n.running) + n.runningTasks + n.kids.reduce((a, k) => a + live(k), 0)
  const finish = (n: GraphNode, detached: boolean): number => {
    n.detached = detached
    n.tasks.sort(byRunningThenAge)
    n.runningTasks = n.tasks.filter((t) => t.status === 'running').length
    n.total = n.kids.reduce((a, k) => a + 1 + finish(k, detached), 0)
    return n.total
  }
  for (const n of tops) finish(n, !slots[turnOf[n.id]!]!.turn.line)
  for (const s of Object.values(slots)) s.tasks.sort(byRunningThenAge)
  const liveIn = (s: TurnSlot): number =>
    s.kids.reduce((a, k) => a + live(k), 0) +
    s.tasks.filter((t) => t.status === 'running').length +
    s.next.reduce((a, x) => a + liveIn(x), 0)

  // one column: the current version, or the one picked, at each fork
  const column: GraphTurnItem[] = []
  let level = roots
  let fork = ''
  while (level.length) {
    const ks = [...level].sort((a, b) => a.turn.at - b.turn.at)
    const onLine = ks.findIndex((k) => k.turn.line)
    const cur = onLine < 0 ? ks.length - 1 : onLine
    const picked = ks.findIndex((k) => k.turn.id === pick[fork])
    const shown = picked < 0 ? cur : picked
    const s = ks[shown]!
    column.push({
      kind: 'turn',
      id: s.turn.id,
      turn: s.turn,
      kids: s.kids.sort((a, b) => a.branch.created_at - b.branch.created_at),
      tasks: s.tasks,
      ver:
        ks.length > 1
          ? {
              fork,
              ids: ks.map((k) => k.turn.id),
              shown,
              current: cur,
              regen: s.turn.regen,
              others: ks.reduce((a, k, i) => (i === shown ? a : a + liveIn(k)), 0),
            }
          : null,
      off: !s.turn.line,
    })
    fork = s.turn.id
    level = s.next
  }

  // runs of messages that started nothing fold into one quiet line; the
  // first and the last message always show
  const items: GraphItem[] = []
  const quiet = (i: number): boolean => {
    const it = column[i]!
    return i > 0 && i < column.length - 1 && !it.kids.length && !it.tasks.length && !it.ver
  }
  for (let i = 0; i < column.length;) {
    let j = i
    while (j < column.length && quiet(j)) j++
    if (j - i >= 2) {
      const run = column.slice(i, j)
      items.push({ kind: 'quiet', id: `q:${run[0]!.id}`, turns: run, off: run[0]!.off })
      i = j
    } else {
      items.push(column[i]!)
      i++
    }
  }

  const parentOf: Record<NodeId, NodeId | null> = {}
  for (const t of graph.turns) parentOf[t.id] = t.parent
  return { items, nodes, turnOf, parentOf, empty: !items.length }
}
