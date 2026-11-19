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
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { t } from '../../i18n'
import { useBranchView } from '../../lib/store'
import type { TimelineUserEntry } from '../../lib/store'
import { preview } from '../../lib/util'
import { contentSegments } from '../../lib/attachments'
import type { Node as RumuNode } from '../../lib/types'

// The scroller, the content column and the composer arrive as getters
// (ConversationView's and ChatView's template refs), not elements: passing a template ref straight
// through a prop trips vue-tsc's HTMLElement unwrap.
const props = withDefaults(
  defineProps<{
    scrollOf: () => HTMLElement | null
    columnOf: () => HTMLElement | null
    composerOf: () => HTMLElement | null
    reveal?: boolean
  }>(),
  { reveal: false },
)
const emit = defineEmits<{ jump: [i: number] }>()
const view = useBranchView()
const { timeline, busy: isBusy } = view

const scrollEl = computed(() => props.scrollOf())
const innerEl = computed(() => props.columnOf())
const composerEl = computed(() => props.composerOf())

/** Track inset, keeping the ends clear of the sheet's fade and corners. */
const RAIL_INSET = 18
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
/** Rows rendered on each side of the front one (the rest are behind). */
const WHEEL_SPAN = 5
/** Scroll over the wheel: px of wheel delta per row (about a notch of a
 *  mouse wheel); snap after it rests. A swipe on the drum turns it by
 *  SWIPE_ROW_DELTA px per row, close to following the finger. */
const WHEEL_ROW_DELTA = 80
const WHEEL_SNAP = 140
const SWIPE_ROW_DELTA = 36

// ---- state: y positions are track coordinates (0 = the layer's top, which
// is the scroll area's top, and the scroll content's origin at scrollTop 0) ----

const metrics = ref({ S: 0, C: 0, L: 0, T: 0, scale: 1, off: 0 })
const viewTop = ref(0)
/** Where the composer reaches the rail, the layer stops above it (px). */
const bottomGap = ref(0)

interface RailDot {
  /** timeline index (the entry's data-entry) */
  i: number
  /** tick center on the track */
  y: number
  /** the bubble's span in scroll-content coordinates (for "current") */
  start: number
  end: number
  label: string
}
const dots = ref<RailDot[]>([])
/** The running turn's node, while rumu works. */
const headY = ref<number | null>(null)

const railHover = ref(false)
const railFocus = ref(false)
const dragging = ref(false)

/** A prompt's one-line label: the words the user typed, with images left out
 *  of the text (their count stands in for an image-only prompt). The
 *  `[image: …]` placeholders belong to text-only readers, not the wheel. */
function promptLabel(e: TimelineUserEntry): string {
  const content = (e.node.body as Extract<RumuNode['body'], { type: 'user' }>).content
  const text = preview(
    contentSegments(content)
      .map((seg) => (seg.type === 'text' ? seg.text : ''))
      .join(''),
    80,
  )
  const images = content.attachments?.length ?? 0
  if (text || !images) return text
  return t('chat.timeline.images', { count: images })
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
  const card = (wrap.firstElementChild as HTMLElement | null) ?? wrap
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
function measure(): void {
  const sc = scrollEl.value
  const inner = innerEl.value
  const parent = layerEl.value?.parentElement
  if (!sc || !inner || !parent) {
    metrics.value = { S: 0, C: 0, L: 0, T: 0, scale: 1, off: 0 }
    dots.value = []
    headY.value = null
    return
  }
  bottomGap.value = composerGap(parent)
  const S = sc.scrollHeight
  const C = sc.clientHeight
  const L = Math.max(0, parent.clientHeight - bottomGap.value)
  const T = Math.max(0, L - RAIL_INSET * 2)
  // A conversation that doesn't overflow maps 1:1 (ticks level with their
  // messages); one that overflows compresses onto the track between the
  // insets (never stretched past 1:1, though the track may be taller).
  const overflow = S > C
  const scale = overflow ? Math.min(1, T / S) : 1
  const off = overflow ? RAIL_INSET : 0
  const mapY = (c: number): number => off + c * scale
  const base = inner.offsetTop

  // ticks: one per prompt. A forward pass pushes them apart to the gap, a
  // backward pass clamps them back into the track.
  const raw: Omit<RailDot, 'y'>[] = []
  const natural: number[] = []
  timeline.value.forEach((e, i) => {
    if (e.kind !== 'user') return
    const el = inner.querySelector<HTMLElement>(`[data-entry="${i}"]`)
    if (!el) return
    // the bubble row opens the entry (context pills may follow under it)
    const start = base + el.offsetTop
    const bubble = el.querySelector<HTMLElement>('[data-path-node]')
    const end = start + (bubble?.offsetHeight ?? el.offsetHeight)
    raw.push({ i, start, end, label: promptLabel(e) })
    natural.push(mapY(start + NODE_Y.user))
  })
  const ys = [...natural]
  const lo = off
  const hi = off + T
  for (let k = 1; k < ys.length; k++) ys[k] = Math.min(hi, Math.max(ys[k], ys[k - 1] + TICK_GAP))
  for (let k = ys.length - 2; k >= 0; k--) ys[k] = Math.max(lo, Math.min(ys[k], ys[k + 1] - TICK_GAP))
  dots.value = raw.map((r, k) => ({ ...r, y: ys[k] }))

  // the head: the running turn's node
  const g = lastGroup.value
  const gEl = g >= 0 ? inner.querySelector<HTMLElement>(`[data-entry="${g}"]`) : null
  headY.value =
    isBusy.value && gEl ? Math.min(hi, Math.max(lo, mapY(base + gEl.offsetTop + NODE_Y.group))) : null

  viewTop.value = sc.scrollTop
  metrics.value = { S, C, L, T, scale, off }
}

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
watch([timeline, isBusy], () => measure(), { flush: 'post' })

defineExpose({ measure })

// ---- drawing ----

const railOn = computed(
  () => props.reveal || railHover.value || railFocus.value || wheelOpen.value || dragging.value,
)

/** Where the conversation ends on the track (the tail of the thread). */
const endY = computed(
  () => metrics.value.off + Math.min(metrics.value.S * metrics.value.scale, metrics.value.T),
)

const baseLine = computed(() => {
  const top = dots.value[0]?.y
  if (top === undefined) return null
  return { top: `${top - 1}px`, height: `${Math.max(0, endY.value - top)}px` }
})

/**
 * The viewport: which slice of the conversation is on screen. Drawn as the
 * stretch of line it covers, inked darker; the drag handle over it is
 * invisible and a little taller, so a short slice stays grabbable.
 */
const viewSpan = computed(() => {
  const { S, C, scale, off, T } = metrics.value
  if (S <= C || T <= 0) return null
  const top = Math.min(off + viewTop.value * scale, off + T)
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
/** The drum's turn, as a fractional index into dots (front row = round). */
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

const maxPos = computed(() => Math.max(0, dots.value.length - 1))
const clampPos = (p: number): number => Math.max(0, Math.min(maxPos.value, p))

/** The front row's prompt (the one a click jumps to), while the wheel is open. */
const frontK = computed(() => Math.round(clampPos(wheelPos.value)))
const hot = computed(() => (wheelOpen.value ? (dots.value[frontK.value]?.i ?? -1) : -1))

/** Ease in-out between two ticks: the drum lingers on each prompt and
 * rolls briskly between them, so the front row is always the nearest. */
function detent(f: number): number {
  return f < 0.5 ? 4 * f ** 3 : 1 - (-2 * f + 2) ** 3 / 2
}

/** A track y → the drum's turn (fractional, between the nearest two ticks). */
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
  wheelPos.value = posAt(y)
}

function openWheel(): void {
  if (!dots.value.length) return
  cancelTimers()
  wheelOpen.value = true
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
  return Math.max(6, Math.min(wheelY.value - WHEEL_H / 2, L - WHEEL_H - 6))
})

/** The rows on the drum's visible face, each turned to its angle. */
const wheelRows = computed(() => {
  if (!wheelOpen.value) return []
  const p = clampPos(wheelPos.value)
  const from = Math.max(0, Math.floor(p) - WHEEL_SPAN)
  const to = Math.min(dots.value.length, Math.ceil(p) + WHEEL_SPAN + 1)
  const rows = []
  for (let k = from; k < to; k++) {
    const a = (k - p) * STEP_DEG
    if (Math.abs(a) >= 90) continue
    const facing = Math.cos((a * Math.PI) / 180)
    rows.push({
      d: dots.value[k],
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
  if (touchPinned) return
  closeTimer = setTimeout(closeWheel, CLOSE_GRACE)
}

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
function pickRow(i: number): void {
  if (dragMoved) {
    dragMoved = false
    return
  }
  pick(i)
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
 * over the scrollbar the rail stands in for. */
function onStripScroll(e: WheelEvent): void {
  const sc = scrollEl.value
  if (!sc) return
  e.preventDefault()
  sc.scrollBy({ top: e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY })
}

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

/** Track point → scroll-content position. */
function unmap(y: number): number {
  const { scale, off } = metrics.value
  return (y - off) / scale
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
    const at = dots.value.findIndex((d) => d.i === currentPrompt.value)
    const k = wheelOpen.value
      ? clampPos(frontK.value + (e.key === 'ArrowDown' ? 1 : -1))
      : at >= 0
        ? at
        : maxPos.value
    wheelPos.value = k
    wheelY.value = dots.value[k]?.y ?? 0
    openWheel()
  } else if ((e.key === 'Enter' || e.key === ' ') && wheelOpen.value) {
    e.preventDefault()
    pick(hot.value)
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
  // With a sub-agent open beside the chat, its rail takes the keys pressed
  // in its panel and the chat's rail the rest.
  if ((t instanceof Element && !!t.closest('[data-agent-panel]')) !== view.sub) return
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
          v-for="d in dots"
          :key="d.i"
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
          class="tl-wheel shadow-pop"
          :style="{ top: `${wheelTop}px`, height: `${WHEEL_H}px` }"
          @pointerenter="onWheelEnter"
          @pointerleave="onWheelLeave"
          @pointerdown="onWheelDown"
          @wheel.prevent="onWheelScroll"
        >
          <span aria-hidden="true" class="tl-wheel-band" :style="{ height: `${ITEM_H}px` }" />
          <div class="tl-wheel-drum">
            <button
              v-for="r in wheelRows"
              :key="r.d.i"
              type="button"
              tabindex="-1"
              class="tl-wheel-row"
              :class="{ 'is-hot': r.k === frontK, 'is-cur': r.d.i === currentPrompt }"
              :style="{ ...r.style, height: `${ITEM_H}px`, marginTop: `${-ITEM_H / 2}px` }"
              :title="r.d.label"
              @click="pickRow(r.d.i)"
            >
              <span class="min-w-0 flex-1 truncate">{{ r.d.label || t('chat.timeline.emptyPrompt') }}</span>
              <span class="tl-wheel-num">{{ r.k + 1 }}</span>
            </button>
          </div>
        </div>
      </Transition>
    </template>
  </div>
</template>
