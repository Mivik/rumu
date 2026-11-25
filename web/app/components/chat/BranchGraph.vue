<script setup lang="ts">
// The branch view: the open session's sub-agents as a diagram. The left
// column is the main line's messages (the current version, or the one picked
// on a message's ‹ k/n › — that only changes what this view shows, never the
// head); what each message started hangs to its right, and what those
// started further right, joined org-chart style: a trunk out of the parent
// and a stub into each child. Background tasks sit inside the card of the
// branch that started them. A card opens its sub-agent beside the chat, a
// task row its output; a message's button shows it in the conversation.
// Runs of messages that started nothing fold into one line (never the first
// or the last), and a text cut to two lines unfolds in place. Narrow, children go below their parent, indented,
// instead of to its right: the same layout with other steps.
//
// Cards are DOM, measured once rendered (a ResizeObserver), then placed; the
// lines are one SVG layer under them.
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { locale, t } from '../../i18n'
import { graph, graphModel, loadGraph, showMessage, watchGraph } from '../../lib/state/graph'
import type { GraphItem, GraphNode, GraphQuietItem, GraphVersion } from '../../lib/state/graph'
import { modelByRef } from '../../lib/state/models'
import { openSide } from '../../lib/state/side'
import { openAgentView, openTaskDetail, showView, state, useBranchView } from '../../lib/store'
import { taskState, taskStatusWord } from '../../lib/tasks'
import type { BranchId, GraphTurn, NodeId, TaskInfo, TaskStatus } from '../../lib/types'
import { fmtDuration, fmtTokens, preview } from '../../lib/util'
import Icon from '../ui/Icon.vue'

/** Below this stage width, children go below their parent. */
const NARROW = 600
const TURN_W = 252
const NODE_W = 292
const X_GAP = 48
const INDENT = 16
const GAP = 12
const TURN_GAP = 20
/** Task rows a card lists before "N more". */
const MAX_TASKS = 4
/** What a sub-agent's card shows of its task, unfolded. */
const TASK_CHARS = 2000

const main = useBranchView()

let stopWatch: (() => void) | null = null
onMounted(() => {
  stopWatch = watchGraph()
})
onBeforeUnmount(() => {
  stopWatch?.()
  ro.disconnect()
  stageRo.disconnect()
  clearInterval(timer)
})

/** The version picked at each fork (`GraphVersion.fork`), for this session. */
const pick = reactive<Record<string, NodeId>>({})
/** Branches whose subtree is folded. */
const folded = reactive(new Set<BranchId>())
/** Quiet runs unfolded, by `GraphQuietItem.id`. */
const opened = reactive(new Set<string>())
/** Cards whose text is cut to two lines, and those unfolded, by card key. */
const clamped = reactive(new Set<string>())
const expanded = reactive(new Set<string>())
watch(
  () => state.sessionId,
  () => {
    for (const k of Object.keys(pick)) delete pick[k]
    folded.clear()
    opened.clear()
    clamped.clear()
    expanded.clear()
    revealed = null
    void loadGraph()
  },
)

/** Durations of what still runs move on. */
const now = ref(Date.now())
const timer = setInterval(() => (now.value = Date.now()), 5000)

const ready = computed(() => graph.session === state.sessionId && !!graph.data && !!state.mainBranch)
const model = computed(() =>
  ready.value
    ? graphModel(
        {
          graph: graph.data!,
          main: state.mainBranch!,
          branches: state.branches,
          tasks: Object.values(state.tasks).filter((tk) => tk.session === state.sessionId),
          busy: state.busy,
        },
        pick,
      )
    : null,
)

interface Card {
  key: string
  item: GraphItem | null
  node: GraphNode | null
  kids: Card[]
}
const tree = computed<Card[]>(() => {
  const m = model.value
  if (!m) return []
  const nodeCard = (n: GraphNode): Card => ({
    key: n.id,
    item: null,
    node: n,
    kids: folded.has(n.id) ? [] : n.kids.map(nodeCard),
  })
  const itemCard = (it: GraphItem): Card => ({
    key: it.id,
    item: it,
    node: null,
    kids: it.kind === 'turn' ? it.kids.map(nodeCard) : [],
  })
  const out: Card[] = []
  for (const it of m.items) {
    out.push(itemCard(it))
    if (it.kind === 'quiet' && opened.has(it.id)) out.push(...it.turns.map(itemCard))
  }
  return out
})
const cards = computed(() => {
  const out: Card[] = []
  const walk = (c: Card): void => {
    out.push(c)
    c.kids.forEach(walk)
  }
  tree.value.forEach(walk)
  return out
})

// ---- measuring and placing

const heights = reactive<Record<string, number>>({})
const ro = new ResizeObserver((entries) => {
  for (const e of entries) {
    const el = e.target as HTMLElement
    const key = el.dataset.k
    if (!key) continue
    heights[key] = Math.ceil(el.offsetHeight)
    // unfolded, the text shows whole: whether it was cut stays as it was
    const ttl = el.querySelector<HTMLElement>('.g-ttl')
    if (!ttl || expanded.has(key)) continue
    if (ttl.scrollHeight > ttl.clientHeight + 1) clamped.add(key)
    else clamped.delete(key)
  }
})
const els = new Map<string, HTMLElement>()
function bind(key: string, el: unknown): void {
  const old = els.get(key)
  if (old === el) return
  if (old) ro.unobserve(old)
  if (el instanceof HTMLElement) {
    els.set(key, el)
    ro.observe(el)
  } else els.delete(key)
}

const scroller = ref<HTMLElement | null>(null)
const stageW = ref(0)
const stageRo = new ResizeObserver(([e]) => {
  if (e) stageW.value = Math.floor(e.contentRect.width)
})
watch(scroller, (el, old) => {
  if (old) stageRo.unobserve(old)
  if (el) stageRo.observe(el)
})
const narrow = computed(() => stageW.value - 48 < NARROW)
const pad = computed(() => (narrow.value ? 12 : 24))

interface Box {
  x: number
  y: number
  w: number
  h: number
}
const layout = computed(() => {
  const box: Record<string, Box> = {}
  const full = Math.max(240, stageW.value - 2 * pad.value)
  let y = 0
  if (!narrow.value) {
    // a card sits level with its first child; siblings stack
    const place = (c: Card, x: number, top: number): number => {
      const w = c.node ? NODE_W : TURN_W
      const h = heights[c.key] ?? 0
      box[c.key] = { x, y: top, w, h }
      let cy = top
      for (const k of c.kids) cy += place(k, x + w + X_GAP, cy) + GAP
      return Math.max(h, c.kids.length ? cy - top - GAP : 0)
    }
    for (const c of tree.value) y += place(c, 0, y) + TURN_GAP
  } else {
    // children below their parent, indented
    const place = (c: Card, depth: number, top: number): number => {
      const x = depth * INDENT
      const h = heights[c.key] ?? 0
      box[c.key] = { x, y: top, w: full - x, h }
      let cy = top + h
      for (const k of c.kids) cy = place(k, depth + 1, cy + GAP)
      return cy
    }
    for (const c of tree.value) y = place(c, 0, y) + TURN_GAP
  }
  let width = 0
  for (const b of Object.values(box)) width = Math.max(width, b.x + b.w)
  return { box, width, height: Math.max(0, y - TURN_GAP) }
})

interface Edge {
  d: string
  kind: 'spine' | 'trunk' | 'stub'
  /** A stub's colour; `side` dashes it. */
  color?: string
  side?: boolean
}
const edges = computed(() => {
  const { box } = layout.value
  const out: Edge[] = []
  const dots: { x: number; y: number }[] = []
  // the main line through its messages; narrow, it goes on from where the
  // line down to a message's children ends
  const sx = narrow.value ? 8 : 12
  let prev: number | null = null
  for (const c of tree.value) {
    const b = box[c.key]!
    if (prev != null) out.push({ d: `M${sx},${prev} V${b.y}`, kind: 'spine' })
    const last = c.kids[c.kids.length - 1]
    prev = narrow.value && last ? box[last.key]!.y + 16 : b.y + b.h
  }
  for (const c of cards.value) {
    if (!c.kids.length) continue
    const b = box[c.key]!
    const last = box[c.kids[c.kids.length - 1]!.key]!.y + 16
    const stub = (k: Card, tx: number): void => {
      const kb = box[k.key]!
      const n = k.node!
      const y2 = kb.y + 16
      out.push({
        d: `M${tx},${y2 - 6} Q${tx},${y2} ${tx + 6},${y2} H${kb.x}`,
        kind: 'stub',
        color: n.running ? 'var(--pine)' : `var(--g${n.accent})`,
        side: n.branch.kind.type === 'side',
      })
    }
    if (!narrow.value) {
      const x1 = b.x + b.w
      const y1 = b.y + 16
      const tx = x1 + 18
      dots.push({ x: x1, y: y1 })
      out.push({ d: `M${x1},${y1} H${tx} V${Math.max(y1, last - 6)}`, kind: 'trunk' })
      for (const k of c.kids) {
        const kb = box[k.key]!
        if (kb.y + 16 <= y1) {
          const n = k.node!
          out.push({
            d: `M${tx},${y1} H${kb.x}`,
            kind: 'stub',
            color: n.running ? 'var(--pine)' : `var(--g${n.accent})`,
            side: n.branch.kind.type === 'side',
          })
        } else stub(k, tx)
      }
    } else {
      const tx = b.x + 8
      out.push({ d: `M${tx},${b.y + b.h} V${last - 6}`, kind: 'trunk' })
      for (const k of c.kids) stub(k, tx)
    }
  }
  return { lines: out, dots }
})

// ---- pointing at a sub-agent (`&agent=`)

/** The cards the view points at: the sub-agent open beside it, and the one
 *  the address names. */
const isSelected = (id: BranchId): boolean => id === state.agentView || id === state.viewAgent
let revealed: string | null = null

/** Show `id`: unfold the branches above it and pick the versions that
 *  hold it. */
function reveal(id: BranchId): void {
  const m = model.value
  if (!m) return
  let top = id
  for (let guard = 0; guard < 100; guard++) {
    const b = state.branches[top]
    if (!b || b.kind.type === 'main' || !m.nodes[b.kind.parent_branch]) break
    top = b.kind.parent_branch
    folded.delete(top)
  }
  const turns = new Map(graph.data!.turns.map((tn) => [tn.id, tn]))
  let turn: NodeId | null = m.turnOf[top] ?? null
  if (!turn || turns.get(turn)?.line) return
  for (let guard = 0; turn && guard < 10000; guard++) {
    const parent: NodeId | null = m.parentOf[turn] ?? null
    pick[parent ?? ''] = turn
    turn = parent
  }
}
watch(
  [() => state.viewAgent, model],
  ([id]) => {
    if (!id || id === revealed || !model.value?.nodes[id]) return
    revealed = id
    reveal(id)
    // once placed
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const b = layout.value.box[id]
        const el = scroller.value
        if (!b || !el) return
        el.scrollTo({
          top: Math.max(0, b.y + pad.value - el.clientHeight / 3),
          left: Math.max(0, b.x + pad.value - 24),
          behavior: 'smooth',
        })
      }),
    )
  },
  { immediate: true },
)

// ---- actions

function step(v: GraphVersion, by: number): void {
  const id = v.ids[v.shown + by]
  if (id) pick[v.fork] = id
}
function back(v: GraphVersion): void {
  delete pick[v.fork]
}
function toggle<T>(set: Set<T>, id: T): void {
  if (set.has(id)) set.delete(id)
  else set.add(id)
}
/** Each main-line message's turn number (1-based), by its node. */
const turnNumber = computed(() => {
  const out = new Map<NodeId, number>()
  for (const tn of graph.data?.turns ?? []) if (tn.line) out.set(tn.user, out.size + 1)
  return out
})
function jump(turn: GraphTurn): void {
  const n = turnNumber.value.get(turn.user)
  if (n) void showMessage(turn.user, n)
}
function open(n: GraphNode): void {
  const k = n.branch.kind
  if (k.type === 'agent') return openAgentView(n.id)
  if (k.type !== 'side') return
  // a side question opens where it was asked
  if (k.parent_branch === state.mainBranch) {
    showView('chat')
    openSide(main.aside, n.id)
  } else openAgentView(k.parent_branch)
}

// ---- what cards say

function clock(at: number): string {
  const d = new Date(at)
  const opts: Intl.DateTimeFormatOptions = { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }
  if (at < new Date().setHours(0, 0, 0, 0)) Object.assign(opts, { month: 'numeric', day: 'numeric' })
  return d.toLocaleString(locale.value, opts)
}
function nodeState(n: GraphNode): TaskStatus {
  if (n.running) return 'running'
  return n.task ? taskState(n.task) : 'done'
}
function kindLabel(n: GraphNode): string {
  const k = n.branch.kind
  if (k.type !== 'agent') return t('chat.graph.side')
  const how = n.task ? t('chat.graph.bg') : t('chat.graph.fg')
  return k.inherit?.context === 'full' ? `${how} · ${t('chat.graph.fork')}` : how
}
function title(n: GraphNode): string {
  const k = n.branch.kind
  if (k.type === 'agent') {
    const task = k.task.trim()
    return task.length > TASK_CHARS ? `${task.slice(0, TASK_CHARS)}…` : task
  }
  return n.sum?.text || (k.type === 'side' && k.quote ? preview(k.quote.text, 200) : t('chat.graph.side'))
}
function span(n: GraphNode): string {
  const end = n.running ? now.value : (n.sum?.last_at ?? n.branch.created_at)
  return `${clock(n.branch.created_at)} · ${fmtDuration(Math.max(0, end - n.branch.created_at))}`
}
function meta(n: GraphNode): string[] {
  if (n.branch.kind.type !== 'agent') return []
  const out: string[] = []
  if (n.sum) {
    out.push(t('chat.graph.calls', { count: n.sum.calls }), t('chat.graph.tools', { count: n.sum.tools }))
  }
  const u = state.agentUsage[n.id]
  if (u?.calls) out.push(t('chat.graph.tokens', { input: fmtTokens(u.input), output: fmtTokens(u.output) }))
  return out
}
function modelName(n: GraphNode): string | null {
  const ref = n.sum?.model
  return ref ? (modelByRef(ref)?.name ?? ref) : null
}
function taskName(tk: TaskInfo): string {
  return tk.kind.type === 'agent' ? t('chat.graph.agentTask') : tk.kind.name
}
function taskText(tk: TaskInfo): string {
  return tk.kind.type === 'agent' ? tk.summary : tk.kind.args_summary
}
const DOT: Record<TaskStatus, string> = {
  running: 'dot-live dot-run bg-pine',
  done: 'bg-ink-3/60',
  failed: 'bg-danger',
  cancelled: 'border-[1.5px] border-ink-3',
  lost: 'bg-warn',
}
function quietText(it: GraphQuietItem): string {
  const a = it.turns[0]!.turn.at
  const b = it.turns[it.turns.length - 1]!.turn.at
  const range = it.turns.length > 1 ? `${clock(a)}–${clock(b)}` : clock(a)
  return `${t('chat.graph.quiet', { count: it.turns.length })} · ${range}`
}
</script>

<template>
  <div ref="scroller" class="g-canvas min-h-0 flex-1 overflow-auto">
    <div v-if="!ready" class="py-10 text-center text-meta text-ink-3">
      <template v-if="graph.failed">
        {{ t('chat.graph.failed') }}
        <button type="button" class="ml-2 font-semibold text-ink-2 hover:text-ink" @click="loadGraph()">
          {{ t('chat.graph.retry') }}
        </button>
      </template>
      <template v-else>{{ t('chat.graph.loading') }}</template>
    </div>
    <div
      v-else-if="model?.empty"
      class="flex flex-col items-center gap-2 px-6 py-16 text-center text-meta text-ink-3"
    >
      <Icon name="branches" :size="22" />
      {{ t('chat.graph.empty') }}
    </div>
    <div
      v-else
      class="g-stage relative"
      :class="{ narrow }"
      :style="{ width: `${layout.width}px`, height: `${layout.height}px`, margin: `${pad}px` }"
    >
      <svg
        class="pointer-events-none absolute top-0 left-0 overflow-visible"
        :width="layout.width"
        :height="layout.height"
      >
        <path
          v-for="(e, i) in edges.lines"
          :key="i"
          :d="e.d"
          fill="none"
          :class="`g-${e.kind}`"
          :style="e.color ? { stroke: e.color } : undefined"
          :stroke-dasharray="e.side ? '3 4' : undefined"
        />
        <circle v-for="(p, i) in edges.dots" :key="`d${i}`" :cx="p.x" :cy="p.y" r="3" class="g-joint" />
      </svg>

      <template v-for="c in cards" :key="c.key">
        <!-- a message (its versions stacked behind it) -->
        <div
          v-if="c.item?.kind === 'turn'"
          :ref="(el) => bind(c.key, el)"
          :data-k="c.key"
          class="g-card g-turn"
          :class="{
            stack: c.item.ver,
            stack3: c.item.ver && c.item.ver.ids.length > 2,
            off: c.item.off,
            hidden: heights[c.key] == null,
          }"
          :style="{
            left: `${layout.box[c.key]?.x ?? 0}px`,
            top: `${layout.box[c.key]?.y ?? 0}px`,
            width: `${layout.box[c.key]?.w ?? TURN_W}px`,
          }"
        >
          <div class="g-hd">
            <span class="truncate"
              >{{ t('chat.graph.user')
              }}<template v-if="c.item.ver">
                · {{ c.item.ver.regen ? t('chat.graph.regen') : t('chat.graph.edit') }}</template
              ></span
            >
            <span class="flex-1" />
            <span v-if="c.item.ver" class="g-ver">
              <button
                type="button"
                :disabled="c.item.ver.shown === 0"
                :title="t('chat.graph.prev')"
                :aria-label="t('chat.graph.prev')"
                @click="step(c.item.ver, -1)"
              >
                ‹
              </button>
              <span class="tabular-nums">{{ c.item.ver.shown + 1 }}/{{ c.item.ver.ids.length }}</span>
              <button
                type="button"
                :disabled="c.item.ver.shown === c.item.ver.ids.length - 1"
                :title="t('chat.graph.next')"
                :aria-label="t('chat.graph.next')"
                @click="step(c.item.ver, 1)"
              >
                ›
              </button>
              <i
                v-if="c.item.ver.others"
                class="g-livedot"
                :title="t('chat.graph.othersLive', { count: c.item.ver.others })"
              />
            </span>
            <span class="flex-none tabular-nums">{{ clock(c.item.turn.at) }}</span>
            <button
              v-if="c.item.turn.line"
              type="button"
              class="g-jump"
              :title="t('chat.graph.jump')"
              :aria-label="t('chat.graph.jump')"
              @click="jump(c.item.turn)"
            >
              <Icon name="external" :size="13" />
            </button>
          </div>
          <button
            v-if="c.item.ver && c.item.ver.shown !== c.item.ver.current"
            type="button"
            class="g-back"
            @click="back(c.item.ver)"
          >
            {{ t('chat.graph.back') }}
          </button>
          <div class="g-ttl" :class="{ open: expanded.has(c.key) }">
            {{ c.item.turn.text || t('chat.graph.noText') }}
          </div>
          <button v-if="clamped.has(c.key)" type="button" class="g-more" @click="toggle(expanded, c.key)">
            {{ expanded.has(c.key) ? t('chat.graph.collapse') : t('chat.graph.expand') }}
          </button>
          <div v-if="c.item.tasks.length" class="g-tasks">
            <button
              v-for="tk in c.item.tasks.slice(0, MAX_TASKS)"
              :key="tk.id"
              type="button"
              class="g-task"
              :title="`${tk.id}\n${taskText(tk)}`"
              @click="openTaskDetail(tk.id)"
            >
              <span class="g-dot" :class="DOT[taskState(tk)]" />
              <span class="flex-none text-ink-3">{{ taskName(tk) }}</span>
              <span class="min-w-0 flex-1 truncate text-ink-2">{{ taskText(tk) }}</span>
              <span
                v-if="!['running', 'done'].includes(taskState(tk))"
                class="g-pill"
                :class="taskState(tk)"
                >{{ taskStatusWord(taskState(tk)) }}</span
              >
            </button>
            <div v-if="c.item.tasks.length > MAX_TASKS" class="g-task more">
              {{ t('chat.graph.more', { count: c.item.tasks.length - MAX_TASKS }) }}
            </div>
          </div>
        </div>

        <!-- messages that started nothing -->
        <button
          v-else-if="c.item?.kind === 'quiet'"
          :ref="(el) => bind(c.key, el)"
          :data-k="c.key"
          type="button"
          class="g-card g-quiet"
          :class="{ off: c.item.off, hidden: heights[c.key] == null }"
          :style="{
            left: `${layout.box[c.key]?.x ?? 0}px`,
            top: `${layout.box[c.key]?.y ?? 0}px`,
            width: `${layout.box[c.key]?.w ?? TURN_W}px`,
          }"
          :title="opened.has(c.key) ? t('chat.graph.collapse') : t('chat.graph.expand')"
          :aria-expanded="opened.has(c.key)"
          @click="toggle(opened, c.key)"
        >
          <Icon name="chevron" :size="12" class="g-chev" :class="{ open: opened.has(c.key) }" />
          {{ quietText(c.item) }}
        </button>

        <!-- a sub-agent or side question -->
        <div
          v-else-if="c.node"
          :ref="(el) => bind(c.key, el)"
          :data-k="c.key"
          role="button"
          tabindex="0"
          class="g-card g-node"
          :class="{
            side: c.node.branch.kind.type === 'side',
            ended: !c.node.running,
            stray: c.node.detached && (c.node.running || c.node.runningTasks),
            sel: isSelected(c.node.id),
            hidden: heights[c.key] == null,
          }"
          :style="{
            left: `${layout.box[c.key]?.x ?? 0}px`,
            top: `${layout.box[c.key]?.y ?? 0}px`,
            width: `${layout.box[c.key]?.w ?? NODE_W}px`,
            '--accent': `var(--g${c.node.accent})`,
          }"
          @click="open(c.node)"
          @keydown.enter.self="open(c.node)"
        >
          <div class="g-hd">
            <span class="g-dot" :class="DOT[nodeState(c.node)]" />
            <span class="truncate">{{ kindLabel(c.node) }}</span>
            <span class="flex-1" />
            <span v-if="c.node.detached && (c.node.running || c.node.runningTasks)" class="g-pill bad">{{
              t('chat.graph.detached')
            }}</span>
            <span v-else-if="!c.node.running && c.node.runningTasks" class="g-pill">{{
              t('chat.graph.ended', { count: c.node.runningTasks })
            }}</span>
            <span
              v-else-if="!['running', 'done'].includes(nodeState(c.node))"
              class="g-pill"
              :class="nodeState(c.node)"
              >{{ taskStatusWord(nodeState(c.node)) }}</span
            >
            <span class="flex-none tabular-nums">{{ span(c.node) }}</span>
          </div>
          <div class="g-ttl strong" :class="{ open: expanded.has(c.key) }">{{ title(c.node) }}</div>
          <button
            v-if="clamped.has(c.key)"
            type="button"
            class="g-more"
            @click.stop="toggle(expanded, c.key)"
          >
            {{ expanded.has(c.key) ? t('chat.graph.collapse') : t('chat.graph.expand') }}
          </button>
          <div v-if="meta(c.node).length || modelName(c.node)" class="g-meta">
            <span v-for="(m, i) in meta(c.node)" :key="i">{{ m }}</span>
            <span v-if="modelName(c.node)" class="wrap-anywhere">{{ modelName(c.node) }}</span>
          </div>
          <div v-if="c.node.tasks.length" class="g-tasks">
            <button
              v-for="tk in c.node.tasks.slice(0, MAX_TASKS)"
              :key="tk.id"
              type="button"
              class="g-task"
              :title="`${tk.id}\n${taskText(tk)}`"
              @click.stop="openTaskDetail(tk.id)"
            >
              <span class="g-dot" :class="DOT[taskState(tk)]" />
              <span class="flex-none text-ink-3">{{ taskName(tk) }}</span>
              <span class="min-w-0 flex-1 truncate text-ink-2">{{ taskText(tk) }}</span>
              <span
                v-if="!['running', 'done'].includes(taskState(tk))"
                class="g-pill"
                :class="taskState(tk)"
                >{{ taskStatusWord(taskState(tk)) }}</span
              >
            </button>
            <div v-if="c.node.tasks.length > MAX_TASKS" class="g-task more">
              {{ t('chat.graph.more', { count: c.node.tasks.length - MAX_TASKS }) }}
            </div>
          </div>
          <button
            v-if="c.node.kids.length"
            type="button"
            class="g-fold"
            :title="
              folded.has(c.node.id) ? t('chat.graph.unfold', { count: c.node.total }) : t('chat.graph.fold')
            "
            :aria-label="
              folded.has(c.node.id) ? t('chat.graph.unfold', { count: c.node.total }) : t('chat.graph.fold')
            "
            @click.stop="toggle(folded, c.node.id)"
          >
            {{ folded.has(c.node.id) ? `+${c.node.total}` : '–' }}
          </button>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.g-canvas {
  background-image: radial-gradient(var(--line) 1px, transparent 1px);
  background-size: 18px 18px;
}
.g-stage {
  /* each sub-agent's colour, by `GraphNode.accent` */
  --g1: var(--agent);
  --g2: var(--tool);
  --g3: var(--memory);
  --g4: var(--brand);
}
.g-spine {
  stroke: var(--ink-3);
  stroke-width: 2;
  stroke-dasharray: 2 4;
}
.g-trunk {
  stroke: var(--ink-3);
  stroke-width: 1.5;
}
.g-stub {
  stroke-width: 1.5;
}
.g-joint {
  fill: var(--ink-3);
}

.g-card {
  position: absolute;
  padding: 8px 10px;
  border: 1px solid var(--line);
  border-radius: 10px;
  background: var(--surface);
  font-size: var(--text-meta);
  line-height: 1.45;
}
.g-card.hidden {
  visibility: hidden;
}
.g-turn {
  background: var(--surface-2);
  border-style: dashed;
}
/* a message with several versions: the others peek out behind it */
.g-turn.stack {
  border-style: solid;
  box-shadow:
    4px 4px 0 -1px var(--surface-2),
    4px 4px 0 0 var(--line);
}
.g-turn.stack3 {
  box-shadow:
    4px 4px 0 -1px var(--surface-2),
    4px 4px 0 0 var(--line),
    8px 8px 0 -1px var(--surface-2),
    8px 8px 0 0 var(--line);
}
/* a version other than the current one */
.g-turn.off,
.g-quiet.off {
  border-color: color-mix(in srgb, var(--brand) 55%, var(--line));
}
.g-quiet {
  display: flex;
  align-items: center;
  padding: 4px 10px 4px 6px;
  border-style: dashed;
  background: transparent;
  color: var(--ink-3);
  font-size: var(--text-micro);
  gap: 4px;
  text-align: left;
}
.g-quiet:hover {
  border-color: var(--ink-3);
  color: var(--ink-2);
}
.g-chev {
  flex: none;
  transition: transform 0.15s;
}
.g-chev.open {
  transform: rotate(90deg);
}

.g-node {
  cursor: pointer;
  box-shadow: 0 1px 2px rgb(0 0 0 / 0.04);
  transition:
    box-shadow 0.15s,
    border-color 0.15s;
}
.g-node::before {
  content: '';
  position: absolute;
  top: 10px;
  bottom: 10px;
  left: -1px;
  width: 3px;
  border-radius: 2px;
  background: var(--accent);
}
.g-node:hover {
  box-shadow: 0 2px 10px rgb(0 0 0 / 0.08);
}
.g-node:focus-visible {
  outline: 2px solid var(--ink-3);
  outline-offset: 1px;
}
.g-node.side {
  background: var(--surface-2);
}
.g-node.side::before {
  background: var(--ink-3);
}
.g-node.stray {
  border-color: color-mix(in srgb, var(--danger) 55%, var(--line));
}
.g-node.sel {
  border-color: var(--ink-2);
  box-shadow: 0 0 0 1px var(--ink-2);
}

/* the time drops to a line of its own when a status crowds it out */
.g-hd {
  display: flex;
  min-width: 0;
  flex-wrap: wrap;
  align-items: center;
  gap: 2px 6px;
  color: var(--ink-3);
  font-size: var(--text-micro);
}
.g-dot {
  width: 7px;
  height: 7px;
  flex: none;
  border-radius: 50%;
}
.g-ttl {
  display: -webkit-box;
  margin-top: 3px;
  overflow: hidden;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  color: var(--ink-2);
  overflow-wrap: anywhere;
}
.g-ttl.open {
  display: block;
  white-space: pre-wrap;
}
.g-ttl.strong {
  color: var(--ink);
  font-weight: 600;
}
.g-meta {
  display: flex;
  flex-wrap: wrap;
  margin-top: 3px;
  color: var(--ink-3);
  font-size: var(--text-micro);
  column-gap: 8px;
}
.g-tasks {
  display: flex;
  flex-direction: column;
  margin-top: 6px;
  gap: 3px;
}
.g-task {
  display: flex;
  min-width: 0;
  align-items: center;
  padding: 2px 6px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--ink) 4%, transparent);
  font-size: var(--text-micro);
  gap: 6px;
  text-align: left;
}
button.g-task:hover {
  background: color-mix(in srgb, var(--ink) 8%, transparent);
}
.g-task.more {
  background: transparent;
  color: var(--ink-3);
}
.g-pill {
  flex: none;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--surface-2);
  color: var(--ink-2);
  font-size: 10.5px;
  line-height: 15px;
  white-space: nowrap;
}
.g-pill.failed,
.g-pill.bad {
  background: var(--danger-soft);
  color: var(--danger);
}
.g-pill.lost {
  background: color-mix(in srgb, var(--warn) 16%, transparent);
  color: var(--warn);
}

.g-ver {
  position: relative;
  display: inline-flex;
  flex: none;
  align-items: center;
  color: var(--ink-2);
}
.g-ver button {
  padding: 0 4px;
  border-radius: 4px;
  font-size: 13px;
  line-height: 1;
}
.g-ver button:hover:not(:disabled) {
  background: var(--line);
}
.g-ver button:disabled {
  opacity: 0.35;
}
/* something runs in a version not shown */
.g-livedot {
  position: absolute;
  top: -2px;
  right: -3px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--danger);
}
.g-back {
  display: block;
  margin: 4px 0 2px;
  color: var(--brand-strong);
  font-size: var(--text-micro);
}
.g-back:hover {
  text-decoration: underline;
}
.g-more {
  margin-top: 1px;
  color: var(--ink-3);
  font-size: var(--text-micro);
}
.g-more:hover {
  color: var(--ink);
}
.g-jump {
  display: grid;
  flex: none;
  width: 18px;
  height: 18px;
  margin: -2px -4px -2px 0;
  place-items: center;
  border-radius: 4px;
  color: var(--ink-3);
  line-height: 0;
}
.g-jump:hover {
  background: var(--line);
  color: var(--ink);
}
.g-fold {
  position: absolute;
  top: 9px;
  right: -11px;
  z-index: 1;
  display: grid;
  width: 20px;
  height: 20px;
  place-items: center;
  border: 1px solid var(--line);
  border-radius: 50%;
  background: var(--surface);
  color: var(--ink-2);
  font-size: 10px;
}
.g-fold:hover {
  border-color: var(--ink-3);
}
/* narrow: the card fills the width, so the button sits on its foot, where
 * the line down to its children starts */
.narrow .g-fold {
  top: auto;
  right: auto;
  bottom: -11px;
  left: -2px;
}
</style>
