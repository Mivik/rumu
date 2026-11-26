<script setup lang="ts">
// The thread rail: the conversation as a pinned minimap down the sheet's
// right edge, standing in for the scrollbar. It runs the full height of the
// chat body but stops above the composer where the composer reaches the edge.
// Each prompt is a tick at its proportional place; the inked stretch is the
// viewport. Grey marks the rest, ink "you are here" (the current prompt — the
// first of your bubbles on screen — and the viewport), coral only the running
// turn's head.
//
// Pointing at the rail opens a picker wheel beside the pointer: prompts on a
// drum, the one nearest the pointer at the front and highlighted. Moving along
// the rail turns the drum; clicking jumps to the front one. Inside the wheel
// the drum holds still — click a row or scroll to turn it. On touch there is
// no hover, so a tap jumps to the nearest prompt and a long press (or a press
// that slides along the rail) opens the wheel to scrub; lifting leaves it
// pinned: swipe the drum to turn it, tap a row to jump, and a tap anywhere
// outside the rail dismisses it.
//
// Drag the viewport stretch to scroll (a tap jumps); the mouse wheel scrolls
// the conversation; Alt+↑/↓ step between prompts anywhere, and ↑/↓ + Enter
// drive the wheel when the rail has focus. The thread (.thread-mark, keyed off
// the layer's data-thread) shows while rumu works, while scrolling, or while
// pointing at the rail, and fades out otherwise; the hit strip stays live.
//
// A long session loads its earlier part as it is needed: while there is
// more, a fixed stretch of dashed thread above the first tick says so. As
// what loads takes its place, the ticks glide to their new places instead
// of jumping.
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { t } from '../../i18n'
import { fetchPrompts, state, turnCount, turnNumbers, useBranchView } from '../../lib/store'
import type { TimelineUserEntry } from '../../lib/store'
import { preview } from '../../lib/util'
import { prefersReduced } from '../../lib/motion'
import { contentSegments, kindOf } from '../../lib/attachments'
import { prefs } from '../../lib/state/prefs'
import Icon from '../ui/Icon.vue'
import type { Content, Node as RumuNode, NodeId } from '../../lib/types'

// The scroller, the content column and the composer arrive as getters
// (ConversationView's and ChatView's template refs), not elements: passing a template ref straight
// through a prop trips vue-tsc's HTMLElement unwrap.
const props = withDefaults(
  defineProps<{
    scrollOf: () => HTMLElement | null
    columnOf: () => HTMLElement | null
    composerOf: () => HTMLElement | null
    /** The conversation goes back further than is loaded. */
    earlier?: boolean
    /** …or on further. */
    later?: boolean
    reveal?: boolean
  }>(),
  { reveal: false, earlier: false, later: false },
)
const emit = defineEmits<{ jump: [i: number]; top: []; bottom: []; turn: [n: number] }>()
const view = useBranchView()
const { timeline, busy: isBusy } = view

const scrollEl = computed(() => props.scrollOf())
const innerEl = computed(() => props.columnOf())
const composerEl = computed(() => props.composerOf())

/** Track inset, keeping the ends clear of the sheet's fade and corners. */
const RAIL_INSET = 18
/** The dashed stretch for what is not loaded yet. */
const DASH_LEN = 40
/** How far ctrl + wheel over the rail sets the views of conversation it
 *  holds (`prefs.railScreens`): past them, it shows a window that slides
 *  with the page instead. Per px of wheel, the range grows by this factor's
 *  log (a notch, about a fifth). */
const SCREENS_MIN = 4
const SCREENS_MAX = 5000
const ZOOM_PER_PX = 0.0015
/** The wheel's slider for the same range: log-scaled, this many steps. */
const RANGE_STEPS = 1000
/** The mouse wheel over the rail scrolls faster than over the page, as a
 *  scrollbar's track pages: this much of a view per px of wheel (a notch,
 *  about 100px, scrolls most of a view). */
const VIEWS_PER_PX = 0.008
/** Ticks never sit closer than this on the track. */
const TICK_GAP = 4
/** The hit strip's width at the sheet's right edge (the line is its center). */
const STRIP_W = 24
/** Node centers from an entry's top: a prompt's first line, a turn's head
 * line (the cat). */
const NODE_Y = { user: 21, group: 12 } as const
/** The scroll area's edge fades (.scroll-fade): a bubble only under them
 * doesn't count as visible. */
const VIEW_FADE = 20
/** Hover grace: rail → wheel without it closing in between. */
const OPEN_DELAY = 120
const CLOSE_GRACE = 220
/** Pointer travel before a press on the viewport becomes a drag. */
const DRAG_SLOP = 3
/** Touch: a press held this long opens the wheel; one that travels this far
 *  along the rail first is a scrub, not a tap. */
const HOLD_MS = 350
const TAP_SLOP = 8

/** The wheel: row height, the drum's angle per row, and the drum radius
 * that makes rows at the front sit exactly ITEM_H apart. */
const ITEM_H = 30
const STEP_DEG = 18
const RADIUS = ITEM_H / ((STEP_DEG * Math.PI) / 180)
const WHEEL_H = 212
/** The wheel's footer: turn steps and a turn to go to, then the rail's
 *  range (two rows of 32px and a hairline). */
const FOOT_H = 69
/** Rows rendered on each side of the front one (the rest are behind). */
const WHEEL_SPAN = 5
/** Scroll over the wheel: px of wheel delta per row (about a notch of a
 *  mouse wheel); snap after it rests. A swipe on the drum turns it by
 *  SWIPE_ROW_DELTA px per row, close to following the finger. */
const WHEEL_ROW_DELTA = 80
const WHEEL_SNAP = 140
const SWIPE_ROW_DELTA = 36
/** How long the ticks take to glide to where a new layout puts them. */
const GLIDE_MS = 280

// ---- state: y positions are track coordinates (0 = the layer's top, which
// is the scroll area's top, and the scroll content's origin at scrollTop 0) ----

/**
 * The track's geometry. Positions are laid out on a virtual track first:
 * the dashed stretch for what is not loaded above (Dt), the conversation at
 * `scale` (S·scale), the dashed stretch for what is not loaded below (Db) —
 * V long in all, content y at `Dt + y * scale`. When the conversation runs
 * past `prefs.railScreens` views, the scale stops shrinking and the track (T long,
 * from `off`) shows a window of V that slides as the page scrolls:
 * `shift` of it lies above the track.
 */
const metrics = ref({ S: 0, C: 0, L: 0, T: 0, Dt: 0, Db: 0, V: 0, scale: 1, off: 0 })
const viewTop = ref(0)
/** Where the composer reaches the rail, the layer stops above it (px). */
const bottomGap = ref(0)

interface RailDot {
  /** timeline index (the entry's data-entry) */
  i: number
  /** the prompt's node: the same tick across loads that shift indices */
  id: string
  /** tick center on the track */
  y: number
  /** the bubble's span in scroll-content coordinates (for "current") */
  start: number
  end: number
  label: string
}
/** The ticks on the virtual track (`y` there). */
const marks = ref<RailDot[]>([])
/** The running turn's node on the virtual track, while rumu works. */
const headV = ref<number | null>(null)

/** How much of the virtual track lies above the track: none when it fits,
 *  else as much of the overhang as the page is scrolled through. */
const shift = computed(() => {
  const { S, C, T, V } = metrics.value
  if (V <= T || S <= C) return 0
  return Math.min(1, Math.max(0, viewTop.value / (S - C))) * (V - T)
})

/** Virtual track → track. */
function onTrack(v: number): number {
  return metrics.value.off + v - shift.value
}

/** The ticks where they are going, on the track. */
const dots = computed<RailDot[]>(() => marks.value.map((d) => ({ ...d, y: onTrack(d.y) })))

/** How far the viewport stretch moves per px of scroll: the scale, less
 *  what the window slides meanwhile. */
const handleRate = computed(() => {
  const { S, C, T, V, Dt, Db, scale } = metrics.value
  if (V <= T || S <= C) return scale
  return Math.max(0.001, (T - C * scale - Dt - Db) / (S - C))
})

const railHover = ref(false)
const railFocus = ref(false)
const dragging = ref(false)

/** A prompt's one-line label: the words the user typed, with the
 *  attachments left out of the text (their count stands in for a prompt that
 *  carries nothing else). The `[image: …]` placeholders belong to text-only
 *  readers, not the wheel. */
function promptLabel(e: TimelineUserEntry): string {
  return labelOf((e.node.body as Extract<RumuNode['body'], { type: 'user' }>).content)
}

/** Each prompt's text label: measure reads them all on every resize. */
const textLabels = new WeakMap<Content, string>()

function labelOf(content: Content): string {
  let text = textLabels.get(content)
  if (text === undefined) {
    text = preview(
      contentSegments(content)
        .map((seg) => (seg.type === 'text' ? seg.text : ''))
        .join(''),
      80,
    )
    textLabels.set(content, text)
  }
  const attachments = content.attachments ?? []
  if (text || !attachments.length) return text
  const images = attachments.filter((a) => kindOf(a) === 'image').length
  const documents = attachments.filter((a) => kindOf(a) === 'document').length
  if (images && documents) return t('chat.timeline.attachments', { count: attachments.length })
  return images
    ? t('chat.timeline.images', { count: images })
    : t('chat.timeline.documents', { count: documents })
}

/** Only the last group can be the one streaming. */
const lastGroup = computed(() => timeline.value.findLastIndex((e) => e.kind === 'group'))

// ---- measurement ----

const layerEl = shallowRef<HTMLElement | null>(null)

/**
 * Where the composer meets the rail: on a wide sheet the composer card
 * (capped at --col-max, centered) leaves the right edge free and the rail runs
 * to the sheet's foot; once the card reaches the strip, the rail stops at
 * the composer's top.
 */
function composerGap(parent: HTMLElement): number {
  const wrap = composerEl.value
  if (!wrap) return 0
  // the composer card itself: the wrapper's first child is the selection
  // bar's full-width overlay, whose right edge sits on the strip's (a
  // fraction past it in Chrome, which cut the rail off above the composer)
  const card = wrap.querySelector<HTMLElement>(':scope > .composer') ?? wrap
  const reach = card.getBoundingClientRect().right
  const strip = parent.getBoundingClientRect().right - STRIP_W
  return reach > strip ? parent.getBoundingClientRect().bottom - wrap.getBoundingClientRect().top : 0
}

/**
 * Map the conversation onto the track: every prompt's node and the running
 * turn's, scaled and spaced. offsetTop is layout position — unmoved by
 * entrance transforms — and the column's own offsetTop is its place in the
 * scroll content, since the scroller starts at the layer's top.
 */
function measure(animate = true): void {
  const sc = scrollEl.value
  const inner = innerEl.value
  const parent = layerEl.value?.parentElement
  if (!sc || !inner || !parent) {
    metrics.value = { S: 0, C: 0, L: 0, T: 0, Dt: 0, Db: 0, V: 0, scale: 1, off: 0 }
    marks.value = []
    headV.value = null
    return
  }
  bottomGap.value = composerGap(parent)
  const S = sc.scrollHeight
  const C = sc.clientHeight
  const L = Math.max(0, parent.clientHeight - bottomGap.value)
  const T = Math.max(0, L - RAIL_INSET * 2)
  // A conversation that doesn't overflow maps 1:1 (ticks level with their
  // messages); one that overflows compresses onto the track between the
  // insets (never stretched past 1:1, though the track may be taller), down
  // to `prefs.railScreens` views on it.
  const dash = Math.min(DASH_LEN, T / 6)
  const Dt = props.earlier ? dash : 0
  const Db = props.later ? dash : 0
  const overflow = S > C || Dt + Db > 0
  const fit = Math.max(1, T - Dt - Db) / Math.max(1, S)
  const scale = overflow ? Math.min(1, Math.max(fit, T / (prefs.railScreens * Math.max(1, C)))) : 1
  const off = overflow ? RAIL_INSET : 0
  const virt = (c: number): number => Dt + c * scale
  const base = inner.offsetTop
  // the entries' wrappers in one pass: looking each prompt's up on its own
  // walked the column once per prompt, on every resize of a streaming turn
  const wraps = new Map<number, HTMLElement>()
  for (const el of inner.querySelectorAll<HTMLElement>(':scope > [data-entry]')) {
    wraps.set(Number(el.dataset.entry), el)
  }

  // ticks: one per prompt. A forward pass pushes them apart to the gap, a
  // backward pass clamps them back into the conversation's stretch.
  const raw: Omit<RailDot, 'y'>[] = []
  const natural: number[] = []
  timeline.value.forEach((e, i) => {
    if (e.kind !== 'user') return
    const el = wraps.get(i)
    if (!el) return
    // the bubble row opens the entry (context pills may follow under it)
    const start = base + el.offsetTop
    const bubble = el.querySelector<HTMLElement>('[data-path-node]')
    const end = start + (bubble?.offsetHeight ?? el.offsetHeight)
    raw.push({ i, id: e.node.id, start, end, label: promptLabel(e) })
    natural.push(virt(start + NODE_Y.user))
  })
  const ys = [...natural]
  const lo = Dt
  const hi = Dt + S * scale
  for (let k = 1; k < ys.length; k++) ys[k] = Math.min(hi, Math.max(ys[k], ys[k - 1] + TICK_GAP))
  for (let k = ys.length - 2; k >= 0; k--) ys[k] = Math.max(lo, Math.min(ys[k], ys[k + 1] - TICK_GAP))
  if (animate) glideFrom()
  marks.value = raw.map((r, k) => ({ ...r, y: ys[k] }))

  // the head: the running turn's node
  const g = lastGroup.value
  const gEl = wraps.get(g) ?? null
  headV.value =
    isBusy.value && !props.later && gEl
      ? Math.min(hi, Math.max(lo, virt(base + gEl.offsetTop + NODE_Y.group)))
      : null

  viewTop.value = sc.scrollTop
  metrics.value = { S, C, L, T, Dt, Db, V: Dt + S * scale + Db, scale, off }
}

// ---- gliding: a new layout eases the ticks over from where they were ----
// (on the track, where they were drawn: a stretch loaded above moves the
// whole virtual track down and slides the window by about as much, so
// easing on the virtual track would first throw the ticks up the window by
// that much and then glide them back)

/** Where each tick (by node), the dashed stretch above and the
 *  conversation's end were on the track when the glide started; empty when
 *  there is none. */
let from = new Map<string, number>()
/** The glide's progress, 0 → 1 (eased). */
const glide = ref(1)
let glideFrame = 0

const DASH = ' dash'
const END = ' end'
/** The first tick at the glide's start: ticks loaded in above it unroll
 *  from there. */
const TOP = ' top'

/** Start a glide from what is drawn now (call before the layout changes). */
function glideFrom(): void {
  if (!marks.value.length || prefersReduced()) return
  const now = new Map<string, number>()
  for (const d of drawnMarks.value) now.set(d.id, onTrack(d.y))
  now.set(DASH, onTrack(dashV.value))
  now.set(END, onTrack(endV.value))
  now.set(TOP, onTrack(drawnMarks.value[0]!.y))
  from = now
  glide.value = 0
  const t0 = performance.now()
  cancelAnimationFrame(glideFrame)
  const step = (t: number): void => {
    // a frame's time can be from before the glide started
    const f = Math.min(1, Math.max(0, (t - t0) / GLIDE_MS))
    glide.value = 1 - (1 - f) ** 3
    if (f < 1) glideFrame = requestAnimationFrame(step)
    else from = new Map()
  }
  glideFrame = requestAnimationFrame(step)
}

/** A virtual y on its way (on the track) from `key`'s glide start to `y`;
 *  a tick new above the old first one starts from there. */
function glided(key: string, y: number): number {
  if (glide.value >= 1) return y
  const to = onTrack(y)
  let a = from.get(key)
  const top = from.get(TOP)
  if (a === undefined && top !== undefined && to < top) a = top
  return a === undefined ? y : y + (a - to) * (1 - glide.value)
}

/** The ticks where they are drawn, on the virtual track. */
const drawnMarks = computed(() => marks.value.map((d) => ({ ...d, y: glided(d.id, d.y) })))
/** …and on the track, the ones on it. */
const drawnDots = computed(() => {
  const { off, T } = metrics.value
  return drawnMarks.value
    .map((d) => ({ ...d, y: onTrack(d.y) }))
    .filter((d) => d.y >= off - TICK_GAP && d.y <= off + T + TICK_GAP)
})

/** Where the dashed stretch above ends, and the conversation, on the
 *  virtual track. */
const dashV = computed(() => glided(DASH, metrics.value.Dt))
const endV = computed(() => {
  const { Dt, S, scale } = metrics.value
  return glided(END, Dt + S * scale)
})

const headY = computed(() => (headV.value === null ? null : onTrack(headV.value)))

/** Only the composer is watched here: the scroller and the column are laid
 *  out by ConversationView, which sizes the blank under the timeline in its
 *  own resize pass and calls `measure` after it — measured on our own, the
 *  page would be read before that blank settles. */
const resize = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(() => measure()) : null

/** Scroll moves only the viewport stretch and "current" — layout is untouched. */
function onScrollSync(): void {
  if (scrollEl.value) viewTop.value = scrollEl.value.scrollTop
}

watch(
  [scrollEl, innerEl, composerEl, layerEl],
  ([sc, , comp], [oldSc, , oldComp]) => {
    oldSc?.removeEventListener('scroll', onScrollSync)
    if (oldComp) resize?.unobserve(oldComp)
    sc?.addEventListener('scroll', onScrollSync, { passive: true })
    if (comp) resize?.observe(comp)
    measure()
  },
  { immediate: true, flush: 'post' },
)
watch([timeline, isBusy, () => props.earlier, () => props.later], () => measure(), { flush: 'post' })

defineExpose({ measure })

// ---- drawing ----

const railOn = computed(
  () => props.reveal || railHover.value || railFocus.value || wheelOpen.value || dragging.value,
)

/** A stretch of the track from `a` to `b` (track y), cut to the track;
 *  `cut` says whether its top was cut off. */
function onTrackSpan(a: number, b: number): { top: number; height: number; cut: boolean } | null {
  const { off, T } = metrics.value
  const top = Math.max(a, off)
  const bottom = Math.min(b, off + T)
  return bottom > top ? { top, height: bottom - top, cut: a < off - 0.5 } : null
}

/** The thread: from the first tick to the conversation's end (fading out
 *  there, and where the window cuts it above). */
const baseLine = computed(() => {
  const first = drawnMarks.value[0]?.y
  if (first === undefined) return null
  const span = onTrackSpan(onTrack(first) - 1, onTrack(endV.value))
  if (!span) return null
  return {
    top: `${span.top}px`,
    height: `${span.height}px`,
    ...(span.cut
      ? {
          maskImage:
            'linear-gradient(to bottom, transparent, #000 24px, #000 calc(100% - 32px), transparent)',
        }
      : {}),
  }
})

/** What is not loaded yet: dashed, above the first tick and below the end. */
const dashLine = computed(() => {
  const { Dt } = metrics.value
  if (Dt <= 0 && dashV.value <= 0.5) return null
  const first = drawnMarks.value[0]?.y ?? 0
  const span = onTrackSpan(onTrack(0), onTrack(Math.max(dashV.value, first)) - 1)
  return span && { top: `${span.top}px`, height: `${span.height}px` }
})
const dashBelow = computed(() => {
  const { Db } = metrics.value
  if (Db <= 0) return null
  const span = onTrackSpan(onTrack(endV.value) + 3, onTrack(endV.value + Db))
  return span && { top: `${span.top}px`, height: `${span.height}px` }
})

/**
 * The viewport: which slice of the conversation is on screen. Drawn as the
 * stretch of line it covers, inked darker; the drag handle over it is
 * invisible and a little taller, so a short slice stays grabbable. It moves
 * like a scrollbar's thumb, top to bottom of the track over the whole
 * conversation, while a windowed track slides under it.
 */
const viewSpan = computed(() => {
  const { S, C, Dt, scale, off, T } = metrics.value
  if (S <= C || T <= 0) return null
  const top = Math.min(off + Dt + viewTop.value * handleRate.value, off + T)
  const height = Math.min(C * scale, off + T - top)
  return { top, height }
})

const viewLine = computed(() =>
  viewSpan.value
    ? { top: `${viewSpan.value.top}px`, height: `${Math.max(4, viewSpan.value.height)}px` }
    : null,
)

const boxStyle = computed(() => {
  const v = viewSpan.value
  if (!v) return null
  const height = Math.max(24, v.height)
  const top = v.top - (height - v.height) / 2
  return { top: `${top}px`, height: `${height}px` }
})

/**
 * The current prompt: the first of your bubbles on screen. With none in
 * view (deep inside a long reply), the last one above — the prompt that
 * reply answers.
 */
const current = computed(() => {
  const C = metrics.value.C
  const from = viewTop.value + VIEW_FADE
  const to = viewTop.value + C - VIEW_FADE
  let above = -1
  for (const d of dots.value) {
    if (d.end > from && d.start < to) return { i: d.i, visible: true }
    if (d.end <= from) above = d.i
  }
  return { i: above, visible: false }
})
const currentPrompt = computed(() => current.value.i)

// ---- the wheel ----

const wheelOpen = ref(false)
/** The drum's turn, as a fractional index into its rows (front row = round). */
const wheelPos = ref(0)
/** The wheel's center on the track (it follows the pointer). */
const wheelY = ref(0)
/** The pointer is over the wheel: the drum holds still under it. */
const inWheel = ref(false)
/** A touch opened the wheel: it stays until a tap outside (no hover to hold
 *  it, and lifting the finger must not jump blindly). */
let touchPinned = false

let openTimer: ReturnType<typeof setTimeout> | undefined
let holdTimer: ReturnType<typeof setTimeout> | undefined
let closeTimer: ReturnType<typeof setTimeout> | undefined
let snapTimer: ReturnType<typeof setTimeout> | undefined

function cancelTimers(): void {
  clearTimeout(openTimer)
  clearTimeout(closeTimer)
}

// ---- the drum's rows: the ticks, and turns beyond what is loaded ----

/** A row on the drum: a prompt with its tick, or one not loaded (listed by
 *  `branch.prompts`, only its label and turn: picking it loads there). */
interface DrumRow {
  key: NodeId
  label: string
  turn: number | null
  dot: RailDot | null
}

/** The main line's prompts not loaded, by turn, as the drum turned past
 *  either end of the ticks asked for them. */
const titles = shallowRef(new Map<number, { id: NodeId; label: string }>())
const asking = new Set<number>()

/** How many prompts to ask for at a time, and how near the drum's end the
 *  front row gets before it asks. */
const TITLE_BATCH = 50
const TITLE_AHEAD = 4

/** The listed turns that run on from the ticks: before the first, and
 *  after the last while the line goes on unloaded. */
const farRows = computed(() => {
  const before: DrumRow[] = []
  const after: DrumRow[] = []
  const list = dots.value
  if (view.sub || !list.length) return { before, after }
  const first = turnOf(list[0]!, 0)
  const last = turnOf(list[list.length - 1]!, list.length - 1)
  const row = (n: number): DrumRow | null => {
    const got = titles.value.get(n)
    return got ? { key: got.id, label: got.label, turn: n, dot: null } : null
  }
  for (let n = (first ?? 1) - 1, r; first !== null && n >= 1 && (r = row(n)); n--) before.unshift(r)
  for (let n = (last ?? 0) + 1, r; last !== null && props.later && (r = row(n)); n++) after.push(r)
  return { before, after }
})

const drum = computed<DrumRow[]>(() => [
  ...farRows.value.before,
  ...dots.value.map((d, k) => ({ key: d.id, label: d.label, turn: turnOf(d, k), dot: d })),
  ...farRows.value.after,
])
/** The first tick's row on the drum. */
const drumOff = computed(() => farRows.value.before.length)

/** Rows coming or going keep the front one where it is. */
watch(drum, (now, old) => {
  const k = Math.round(Math.max(0, Math.min(old.length - 1, wheelPos.value)))
  const key = old[k]?.key
  const at = key === undefined ? -1 : now.findIndex((r) => r.key === key)
  if (at < 0 || at === k) return
  wheelPos.value += at - k
  dragFromPos += at - k
})

/** The drum turned near an end of what it lists: ask for the turns past it. */
function askTitles(): void {
  if (view.sub || !wheelOpen.value) return
  const rows = drum.value
  const k = frontK.value
  const first = rows[0]?.turn
  const last = rows[rows.length - 1]?.turn
  if (k < TITLE_AHEAD && first != null && first > 1) {
    const from = Math.max(1, first - TITLE_BATCH)
    void ask(from, first - from)
  }
  const total = turnCount.value
  if (k > rows.length - 1 - TITLE_AHEAD && last != null && props.later && (total === null || last < total)) {
    void ask(last + 1, TITLE_BATCH)
  }
}

async function ask(from: number, limit: number): Promise<void> {
  if (asking.has(from)) return
  asking.add(from)
  const got = await fetchPrompts(from, limit)
  asking.delete(from)
  if (!got) return
  const next = new Map(titles.value)
  for (const p of got) next.set(p.turn, { id: p.id, label: labelOf(p.content) })
  titles.value = next
}

/** The listing holds for one line: a head moved anywhere but on down it
 *  (an edit, a version switched, another session) drops it. */
watch(
  () => [state.sessionId, state.mainBranch ? state.branches[state.mainBranch]?.head : null] as const,
  ([session, head], [oldSession, oldHead]) => {
    if (!titles.value.size) return
    let cur: NodeId | null | undefined = head
    for (let up = 0; cur && cur !== oldHead && up < 64; up++) cur = state.nodes[cur]?.parent
    if (session !== oldSession || !cur) titles.value = new Map()
  },
)

const maxPos = computed(() => Math.max(0, drum.value.length - 1))
const clampPos = (p: number): number => Math.max(0, Math.min(maxPos.value, p))

/** The front row (the one a click jumps to), and its prompt's entry while
 *  the wheel is open (-1 for a row not loaded). */
const frontK = computed(() => Math.round(clampPos(wheelPos.value)))
const hot = computed(() => (wheelOpen.value ? (drum.value[frontK.value]?.dot?.i ?? -1) : -1))
watch(frontK, askTitles)

/** Ease in-out between two ticks: the drum lingers on each prompt and
 * rolls briskly between them, so the front row is always the nearest. */
function detent(f: number): number {
  return f < 0.5 ? 4 * f ** 3 : 1 - (-2 * f + 2) ** 3 / 2
}

/** A track y → the drum's turn (fractional, between the nearest two ticks;
 *  an index into the ticks). */
function posAt(y: number): number {
  const list = dots.value
  const n = list.length
  if (!n || y <= list[0].y) return 0
  if (y >= list[n - 1].y) return n - 1
  let k = 0
  while (k < n - 2 && list[k + 1].y <= y) k++
  const span = list[k + 1].y - list[k].y
  return k + (span > 0 ? detent((y - list[k].y) / span) : 0)
}

/** The prompt nearest a track y. */
function nearestAt(y: number): number {
  return dots.value[Math.round(posAt(y))]?.i ?? -1
}

function trackY(e: PointerEvent | MouseEvent): number {
  const rect = layerEl.value?.getBoundingClientRect()
  return rect ? e.clientY - rect.top : 0
}

/** Turn the drum to the pointer, and bring the wheel beside it. */
function follow(y: number): void {
  wheelY.value = y
  wheelPos.value = drumOff.value + posAt(y)
}

function openWheel(): void {
  if (!dots.value.length) return
  cancelTimers()
  wheelOpen.value = true
  askTitles()
}

function closeWheel(): void {
  cancelTimers()
  clearTimeout(snapTimer)
  wheelOpen.value = false
  inWheel.value = false
  touchPinned = false
}

function pick(i: number): void {
  if (i < 0) return
  emit('jump', i)
  closeWheel()
}

const wheelTop = computed(() => {
  const L = metrics.value.L
  const h = WHEEL_H + FOOT_H
  return Math.max(6, Math.min(wheelY.value - h / 2, L - h - 6))
})

/** The rows on the drum's visible face, each turned to its angle. */
const wheelRows = computed(() => {
  if (!wheelOpen.value) return []
  const p = clampPos(wheelPos.value)
  const from = Math.max(0, Math.floor(p) - WHEEL_SPAN)
  const to = Math.min(drum.value.length, Math.ceil(p) + WHEEL_SPAN + 1)
  const rows = []
  for (let k = from; k < to; k++) {
    const a = (k - p) * STEP_DEG
    if (Math.abs(a) >= 90) continue
    const facing = Math.cos((a * Math.PI) / 180)
    rows.push({
      r: drum.value[k]!,
      k,
      style: {
        transform: `translateZ(${-RADIUS}px) rotateX(${-a}deg) translateZ(${RADIUS}px)`,
        opacity: facing ** 2,
      },
    })
  }
  return rows
})

function onWheelEnter(): void {
  inWheel.value = true
  cancelTimers()
}

function onWheelLeave(): void {
  inWheel.value = false
  cancelTimers()
  if (touchPinned || typing()) return
  closeTimer = setTimeout(closeWheel, CLOSE_GRACE)
}

// ---- the wheel's footer: turn by turn, or a turn by number ----

const wheelEl = ref<HTMLElement | null>(null)

/** A turn number is being typed in the wheel: it stays open meanwhile. */
function typing(): boolean {
  const a = document.activeElement
  return a instanceof HTMLInputElement && a.type === 'text' && !!wheelEl.value?.contains(a)
}

/** Leaving the turn field with the pointer elsewhere closes the wheel. */
function onFootOut(): void {
  requestAnimationFrame(() => {
    if (!inWheel.value && !railHover.value && !touchPinned && !typing()) closeWheel()
  })
}

/** A prompt's turn number: along the whole main line (a sub-agent's from
 *  its own start); null while unknown. */
function turnOf(d: RailDot, k: number): number | null {
  return view.sub ? k + 1 : (turnNumbers.value.get(d.id) ?? null)
}

/** The current prompt's turn, and how many there are. */
const curTurn = computed(() => {
  const k = dots.value.findIndex((d) => d.i === currentPrompt.value)
  return k < 0 ? null : turnOf(dots.value[k]!, k)
})
const totalTurns = computed(() => (view.sub ? dots.value.length : turnCount.value))

/** What the turn field holds. */
const turnText = ref('')

/** Go to turn `n` (clamped to the line): the main line loads it if need be;
 *  a sub-agent's are all here. The drum follows. */
function goTurn(n: number): void {
  const total = totalTurns.value
  const to = Math.max(1, total ? Math.min(total, n) : n)
  following = performance.now() + FOLLOW_MS
  if (!view.sub) return emit('turn', to)
  const d = dots.value[to - 1]
  if (d) emit('jump', d.i)
}

/** The previous / next turn from the current one; deep in a reply, the
 *  previous is the prompt it answers. */
function stepTurn(dir: -1 | 1): void {
  const n = curTurn.value
  if (n === null) {
    const first = dots.value[0]
    const f = first ? turnOf(first, 0) : null
    if (dir > 0 && f !== null) goTurn(f)
    return
  }
  goTurn(dir < 0 && !current.value.visible ? n : n + dir)
}

function onTurnEnter(): void {
  const n = parseInt(turnText.value, 10)
  turnText.value = ''
  if (Number.isFinite(n)) goTurn(n)
}

/** After a step or a jump from the footer, the drum's front follows the
 *  current prompt for a while (the scroll there passes others). */
const FOLLOW_MS = 1500
let following = 0
watch(currentPrompt, (i) => {
  if (!wheelOpen.value || performance.now() > following) return
  const k = drum.value.findIndex((r) => r.dot?.i === i)
  if (k >= 0) wheelPos.value = k
})

/** Scrolling over the wheel turns the drum; it settles on a row at rest. */
function onWheelScroll(e: WheelEvent): void {
  const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY
  wheelPos.value = clampPos(wheelPos.value + dy / WHEEL_ROW_DELTA)
  clearTimeout(snapTimer)
  snapTimer = setTimeout(() => (wheelPos.value = Math.round(wheelPos.value)), WHEEL_SNAP)
}

/** A touch swipe on the drum: which pointer, where it started, and whether it
 *  moved far enough to be a turn rather than a row tap. */
let dragPointer: number | null = null
let dragFromY = 0
let dragFromPos = 0
let dragMoved = false

/** Touch has no wheel to scroll, so a vertical swipe on the drum turns it.
 *  The window listeners follow the finger past the wheel's edge; lifting
 *  settles on the nearest row. */
function onWheelDown(e: PointerEvent): void {
  if (e.pointerType === 'mouse' || !e.isPrimary) return
  cancelTimers()
  clearTimeout(snapTimer)
  inWheel.value = true
  dragPointer = e.pointerId
  dragFromY = e.clientY
  dragFromPos = wheelPos.value
  dragMoved = false
  window.addEventListener('pointermove', onWheelMove)
  window.addEventListener('pointerup', onWheelUp)
  window.addEventListener('pointercancel', onWheelUp)
}

function onWheelMove(e: PointerEvent): void {
  if (e.pointerId !== dragPointer) return
  const dy = dragFromY - e.clientY
  if (!dragMoved) {
    if (Math.abs(dy) < DRAG_SLOP) return
    dragMoved = true
  }
  wheelPos.value = clampPos(dragFromPos + dy / SWIPE_ROW_DELTA)
}

function onWheelUp(e: PointerEvent): void {
  if (e.pointerId !== dragPointer) return
  endWheelDrag()
  if (dragMoved) wheelPos.value = Math.round(wheelPos.value)
}

function endWheelDrag(): void {
  dragPointer = null
  window.removeEventListener('pointermove', onWheelMove)
  window.removeEventListener('pointerup', onWheelUp)
  window.removeEventListener('pointercancel', onWheelUp)
}

/** A row's click picks it, unless it is the tail of a swipe on the drum. */
function pickRow(r: DrumRow): void {
  if (dragMoved) {
    dragMoved = false
    return
  }
  if (r.dot) return pick(r.dot.i)
  if (r.turn !== null) emit('turn', r.turn)
  closeWheel()
}

// ---- the strip: hover / click / scrub ----

/** A touch press on the strip, while it may still be a tap (its start y). */
let pressY: number | null = null
/** A touch scrub is underway: the wheel is open and follows the finger. */
let scrubbing = false
/** The last press on the strip was a mouse's: only those click. Touch is
 *  settled by its own pointer events, never by the click that trails them. */
let mousePress = false

function onStripEnter(e: PointerEvent): void {
  railHover.value = true
  cancelTimers()
  if (e.pointerType !== 'mouse') return
  follow(trackY(e))
  if (!wheelOpen.value) openTimer = setTimeout(openWheel, OPEN_DELAY)
}

function onStripLeave(): void {
  railHover.value = false
  if (scrubbing || touchPinned) return
  cancelTimers()
  closeTimer = setTimeout(closeWheel, CLOSE_GRACE)
}

/** A long press (or a slide) turns a touch into a scrub: the wheel opens
 *  beside the finger and stays pinned once it lifts. */
function startScrub(y: number): void {
  clearTimeout(holdTimer)
  pressY = null
  scrubbing = true
  touchPinned = true
  follow(y)
  openWheel()
  navigator.vibrate?.(8)
}

function onStripMove(e: PointerEvent): void {
  if (dragging.value) return
  if (e.pointerType === 'mouse' || scrubbing) follow(trackY(e))
  else if (pressY !== null && Math.abs(trackY(e) - pressY) >= TAP_SLOP) startScrub(trackY(e))
}

function onStripDown(e: PointerEvent): void {
  mousePress = e.pointerType === 'mouse'
  if (mousePress || !e.isPrimary) return
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  const y = trackY(e)
  pressY = y
  holdTimer = setTimeout(() => startScrub(y), HOLD_MS)
}

/** A touch let go before it was held or moved is a tap: jump. */
function onStripUp(e: PointerEvent): void {
  clearTimeout(holdTimer)
  if (pressY !== null) pick(nearestAt(trackY(e)))
  pressY = null
  scrubbing = false
}

function onStripCancel(): void {
  clearTimeout(holdTimer)
  pressY = null
  scrubbing = false
  closeWheel()
}

function onStripClick(e: MouseEvent): void {
  if (mousePress) pick(nearestAt(trackY(e)))
}

/** The mouse wheel over the rail scrolls the conversation, as it would
 * over the scrollbar the rail stands in for; with ctrl (a trackpad pinch
 * too) it sets how much of the conversation the rail holds: down for more,
 * up for less. */
function onStripScroll(e: WheelEvent): void {
  const sc = scrollEl.value
  if (!sc) return
  e.preventDefault()
  const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaMode === 2 ? e.deltaY * 100 : e.deltaY
  if (e.ctrlKey || e.metaKey) {
    setScreens(prefs.railScreens * Math.exp(dy * ZOOM_PER_PX))
    return
  }
  sc.scrollBy({ top: dy * sc.clientHeight * VIEWS_PER_PX })
}

function setScreens(n: number): void {
  prefs.railScreens = Math.min(SCREENS_MAX, Math.max(SCREENS_MIN, n))
}

/** The range on the wheel's slider (log-scaled steps), and back. */
const rangeStep = computed(() =>
  Math.round((RANGE_STEPS * Math.log(prefs.railScreens / SCREENS_MIN)) / Math.log(SCREENS_MAX / SCREENS_MIN)),
)

function onRangeInput(e: Event): void {
  const step = +(e.target as HTMLInputElement).value
  setScreens(SCREENS_MIN * (SCREENS_MAX / SCREENS_MIN) ** (step / RANGE_STEPS))
}

// a new range: the ticks follow at once, as the viewport does
watch(
  () => prefs.railScreens,
  () => measure(false),
)

/** A touch-pinned wheel is dismissed by a press anywhere outside the rail
 * (the strip or the wheel; a row's own click then owns the jump). */
function onDocPointerDown(e: PointerEvent): void {
  if (!touchPinned) return
  const target = e.target as Node
  if (layerEl.value?.contains(target)) return
  closeWheel()
}

watch(wheelOpen, (v) => {
  if (v) document.addEventListener('pointerdown', onDocPointerDown, true)
  else document.removeEventListener('pointerdown', onDocPointerDown, true)
})

// ---- the viewport handle: drag to scroll, tap to jump, hold to open (touch) ----

/** Where the viewport stretch's top at track point `y` puts the page's
 *  scroll (negative on the dashed stretch). */
function unmap(y: number): number {
  const { off, Dt } = metrics.value
  return (y - off - Dt) / handleRate.value
}

/** Past the slop, scrollTop follows the pointer 1:1 in track space; a press
 * that never moves jumps to the nearest prompt. A touch held still opens the
 * wheel as on the rest of the rail (the strip, which the moves bubble to,
 * then scrubs it). */
function onBoxDown(e: PointerEvent): void {
  const sc = scrollEl.value
  if (!sc || !layerEl.value) return
  const { S, C } = metrics.value
  if (S <= C) return
  const el = e.currentTarget as HTMLElement
  el.setPointerCapture(e.pointerId)
  const startY = trackY(e)
  // where in the content the pointer grabbed (kept fixed while dragging)
  const grab = unmap(startY) - sc.scrollTop
  if (e.pointerType !== 'mouse') holdTimer = setTimeout(() => startScrub(startY), HOLD_MS)
  const move = (ev: PointerEvent): void => {
    if (scrubbing) return
    const y = trackY(ev)
    if (!dragging.value) {
      if (Math.abs(y - startY) < DRAG_SLOP) return
      clearTimeout(holdTimer)
      dragging.value = true
      closeWheel()
    }
    sc.scrollTop = Math.max(0, Math.min(S - C, unmap(y) - grab))
  }
  const up = (ev: PointerEvent): void => {
    clearTimeout(holdTimer)
    if (!dragging.value && !scrubbing && ev.type === 'pointerup') pick(nearestAt(trackY(ev)))
    dragging.value = false
    el.removeEventListener('pointermove', move)
    el.removeEventListener('pointerup', up)
    el.removeEventListener('pointercancel', up)
  }
  el.addEventListener('pointermove', move)
  el.addEventListener('pointerup', up)
  el.addEventListener('pointercancel', up)
}

// ---- keyboard ----

/** Focused rail: ↑/↓ open the wheel and turn it, Enter jumps. */
function onStripKey(e: KeyboardEvent): void {
  if (e.altKey || e.isComposing) return
  if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
    e.preventDefault()
    const at = drum.value.findIndex((r) => r.dot !== null && r.dot.i === currentPrompt.value)
    const k = wheelOpen.value
      ? clampPos(frontK.value + (e.key === 'ArrowDown' ? 1 : -1))
      : at >= 0
        ? at
        : drumOff.value + dots.value.length - 1
    wheelPos.value = k
    const dot = drum.value[k]?.dot
    if (dot) wheelY.value = dot.y
    openWheel()
  } else if ((e.key === 'Enter' || e.key === ' ') && wheelOpen.value) {
    e.preventDefault()
    const r = drum.value[frontK.value]
    if (r) pickRow(r)
  }
}

function onStripBlur(): void {
  railFocus.value = false
  // A touch-pinned wheel outlives focus changes (tapping a row moves focus);
  // only the outside press or Escape dismisses it.
  if (!railHover.value && !inWheel.value && !touchPinned) closeWheel()
}

/**
 * Alt+↑/↓ step to the previous / next prompt; Esc closes the wheel. Skipped
 * mid-composition and over a field holding text, so it never fights typing.
 */
function onKeydown(e: KeyboardEvent): void {
  if (e.isComposing) return
  if (e.key === 'Escape' && wheelOpen.value) {
    closeWheel()
    return
  }
  if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return
  const t = e.target
  // With a sub-agent or a side question open over the chat, its rail takes
  // the keys pressed in it and the chat's rail the rest.
  if (!view.holds(t instanceof Element ? t : document.body)) return
  if (t instanceof HTMLElement) {
    if (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA') {
      if ((t as HTMLInputElement).value !== '') return
    } else if (t.isContentEditable && t.textContent !== '') return
  }
  const list = dots.value
  if (!list.length) return
  const at = list.findIndex((d) => d.i === currentPrompt.value)
  let next: number | null = null
  if (e.key === 'ArrowDown') {
    if (at < 0) next = list[0].i
    else if (at < list.length - 1) next = list[at + 1].i
  } else if (at >= 0 && !current.value.visible) {
    // deep in a reply: ↑ goes back to the prompt it answers first
    next = list[at].i
  } else if (at > 0) {
    next = list[at - 1].i
  }
  if (next === null) return
  e.preventDefault()
  emit('jump', next)
}

onMounted(() => window.addEventListener('keydown', onKeydown))

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  document.removeEventListener('pointerdown', onDocPointerDown, true)
  endWheelDrag()
  resize?.disconnect()
  scrollEl.value?.removeEventListener('scroll', onScrollSync)
  cancelTimers()
  clearTimeout(holdTimer)
  clearTimeout(snapTimer)
  cancelAnimationFrame(glideFrame)
})
</script>

<template>
  <div
    ref="layerEl"
    class="tl-rail-layer"
    :data-thread="railOn ? 'on' : 'off'"
    :style="{ bottom: `${bottomGap}px` }"
  >
    <template v-if="dots.length">
      <!-- the hit strip down the sheet's right edge: hover / click / scrub /
           drag surface, alive even while the thread is faded out -->
      <div
        class="tl-rail-hit"
        role="navigation"
        tabindex="0"
        :aria-label="t('chat.timeline.label')"
        aria-keyshortcuts="ArrowUp ArrowDown Enter"
        @pointerenter="onStripEnter"
        @pointerleave="onStripLeave"
        @pointermove="onStripMove"
        @pointerdown="onStripDown"
        @pointerup="onStripUp"
        @pointercancel="onStripCancel"
        @click="onStripClick"
        @contextmenu.prevent
        @wheel="onStripScroll"
        @keydown="onStripKey"
        @focus="railFocus = true"
        @blur="onStripBlur"
      >
        <!-- the part not loaded (or not measured) yet, held open -->
        <span
          v-if="dashLine"
          aria-hidden="true"
          class="thread-mark tl-rail-line tl-rail-dash"
          :style="dashLine"
        />
        <span
          v-if="dashBelow"
          aria-hidden="true"
          class="thread-mark tl-rail-line tl-rail-dash"
          :style="dashBelow"
        />
        <span
          v-if="baseLine"
          aria-hidden="true"
          class="thread-mark tl-rail-line is-tail bg-ink-3/30"
          :style="baseLine"
        />
        <!-- the viewport: the stretch of line on screen, inked darker -->
        <span
          v-if="viewLine"
          aria-hidden="true"
          class="thread-mark tl-rail-line tl-rail-view"
          :class="{ 'is-drag': dragging }"
          :style="viewLine"
        />
        <!-- one tick per prompt (grey; the current one inked, the wheel's
             front one longer still); the head, while rumu works -->
        <span
          v-for="d in drawnDots"
          :key="d.id"
          aria-hidden="true"
          class="thread-mark tl-rail-mark"
          :class="{ 'is-cur': d.i === currentPrompt, 'is-hot': d.i === hot }"
          :style="{ top: `${d.y - 1}px` }"
        />
        <span
          v-if="headY !== null"
          aria-hidden="true"
          class="thread-mark tl-rail-head dot-live"
          :style="{ top: `${headY - 4}px` }"
        />
        <!-- the fixed jumps to the very top and end, in the insets -->
        <template v-if="metrics.off > 0">
          <button
            type="button"
            tabindex="-1"
            class="thread-mark tl-rail-end"
            :style="{ top: '0px', height: `${metrics.off}px` }"
            :title="t('chat.timeline.toStart')"
            :aria-label="t('chat.timeline.toStart')"
            @pointerdown.stop
            @click.stop="emit('top')"
          >
            <Icon name="chev-up" :size="12" />
          </button>
          <button
            type="button"
            tabindex="-1"
            class="thread-mark tl-rail-end"
            :style="{ top: `${metrics.off + metrics.T}px`, height: `${metrics.off}px` }"
            :title="t('chat.timeline.toEnd')"
            :aria-label="t('chat.timeline.toEnd')"
            @pointerdown.stop
            @click.stop="emit('bottom')"
          >
            <Icon name="down" :size="12" />
          </button>
        </template>
        <!-- the viewport's drag handle on top, invisible -->
        <div
          v-if="boxStyle"
          class="tl-rail-box"
          :class="{ 'is-drag': dragging }"
          :style="boxStyle"
          @pointerdown.stop="onBoxDown"
          @click.stop
        />
      </div>

      <!-- the wheel: prompts on a drum, the nearest one at the front -->
      <Transition
        enter-active-class="transition duration-200 ease-soft"
        enter-from-class="translate-x-1.5 opacity-0"
        leave-active-class="transition duration-150"
        leave-to-class="opacity-0"
      >
        <div
          v-if="wheelOpen"
          ref="wheelEl"
          class="tl-wheel shadow-pop"
          :style="{ top: `${wheelTop}px` }"
          @pointerenter="onWheelEnter"
          @pointerleave="onWheelLeave"
          @pointerdown="onWheelDown"
          @wheel.prevent="onWheelScroll"
        >
          <div class="tl-wheel-face" :style="{ height: `${WHEEL_H}px` }">
            <span aria-hidden="true" class="tl-wheel-band" :style="{ height: `${ITEM_H}px` }" />
            <div class="tl-wheel-drum">
              <button
                v-for="{ r, k, style } in wheelRows"
                :key="r.key"
                type="button"
                tabindex="-1"
                class="tl-wheel-row"
                :class="{ 'is-hot': k === frontK, 'is-cur': !!r.dot && r.dot.i === currentPrompt }"
                :style="{ ...style, height: `${ITEM_H}px`, marginTop: `${-ITEM_H / 2}px` }"
                :title="r.label"
                @click="pickRow(r)"
              >
                <span class="min-w-0 flex-1 truncate">{{ r.label || t('chat.timeline.emptyPrompt') }}</span>
                <span class="tl-wheel-num">{{ r.turn ?? '' }}</span>
              </button>
            </div>
          </div>
          <!-- how much of the conversation the rail holds (ctrl + wheel
               over the rail sets it too) -->
          <div class="tl-wheel-foot" @pointerdown.stop @focusout="onFootOut">
            <div class="tl-wheel-tools">
              <button
                type="button"
                class="icon-btn"
                :title="t('chat.timeline.prevTurn')"
                :aria-label="t('chat.timeline.prevTurn')"
                @click="stepTurn(-1)"
              >
                <Icon name="chev-up" :size="14" />
              </button>
              <button
                type="button"
                class="icon-btn"
                :title="t('chat.timeline.nextTurn')"
                :aria-label="t('chat.timeline.nextTurn')"
                @click="stepTurn(1)"
              >
                <Icon name="down" :size="14" />
              </button>
              <label class="tl-wheel-goto">
                {{ t('chat.timeline.turnBefore') }}
                <input
                  v-model="turnText"
                  type="text"
                  inputmode="numeric"
                  enterkeyhint="go"
                  :placeholder="curTurn === null ? '' : String(curTurn)"
                  :aria-label="t('chat.timeline.goTurn')"
                  @keydown.enter.prevent="onTurnEnter"
                />
                {{ t('chat.timeline.turnAfter') }}
                <span v-if="totalTurns" class="tl-wheel-total">/ {{ totalTurns }}</span>
              </label>
            </div>
            <!-- how much of the conversation the rail holds (ctrl + wheel
                 over the rail sets it too) -->
            <label class="tl-wheel-range">
              <span class="flex-none">{{ t('chat.timeline.range') }}</span>
              <input
                type="range"
                class="slider min-w-0 flex-1"
                min="0"
                :max="RANGE_STEPS"
                step="1"
                :value="rangeStep"
                :style="{ '--fill': `${(rangeStep / RANGE_STEPS) * 100}%` }"
                @input="onRangeInput"
              />
              <output>{{ t('chat.timeline.screens', { n: Math.round(prefs.railScreens) }) }}</output>
            </label>
          </div>
        </div>
      </Transition>
    </template>
  </div>
</template>
