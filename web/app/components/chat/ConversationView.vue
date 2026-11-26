<script setup lang="ts">
// The conversation view: the grouped timeline of the shown branch (the main
// line, or a sub-agent beside it — `useBranchView`). Entries come from its `timeline`
// computed (user / context pills / assistant group); the content
// column is capped at --col-max (main.css), centered. On phones and in
// portrait the sheet drops its card edges and becomes the page (main.css).
//
// The page never shrinks under you mid-turn: a blank tail after the timeline
// (the spacer) takes back whatever the column gives up while a turn runs — a
// running call's output folding into its summary line, a fold closed — so
// what you are reading stays put, and growth fills that blank before it moves
// the page. While you follow the end, though, the end is what you read: a
// fold closing under it takes its blank along, so the reply stays in view
// instead of leaving you staring at the blank it left. While a turn runs (and
// is not waiting on a decision), LIVE_ROOM stays free under the reply, so
// whatever shows up next (a tall preview, a tool's output) lands in view;
// otherwise the floor is the column's own height. Between turns the page is
// only the column: when a turn ends the blank it held eases away, and a fold
// opened and closed gives back what it added. Any size change
// of the column or the scroller — not only new tokens — re-pins the end while
// you follow it, after the blank has absorbed what it can: a composer that
// grows (a decision unfolding) eats into the blank, not the page, and gets it
// back when it shrinks. A prompt you just sent eases the view to the end — a
// session switch still lands there at once, and a branch switch keeps your
// place. A clicked `@rumu` node address lands on that message instead of
// the end.
//
// The thread is the rail down the sheet's right edge (TimelineRail), which
// owns its measurement and drawing. This file owns the scroll state and
// jumping; the composer below belongs to the shell (ChatView), which lends
// it to the rail through `composerOf`.
//
// A long conversation is a virtual list (virtualEntries): assistant turns
// away from the view are blanks of their measured height, so the column
// keeps its full height and nothing in view moves as they render back in.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, watchEffect } from 'vue'
import { t } from '../../i18n'
import {
  ensureAgentNodes,
  ensureFullPath,
  goToEnd,
  goToStart,
  goToTurn,
  hasEarlier,
  hasLater,
  loadEarlier,
  loadLater,
  pathNodes,
  state,
  turnNumbers,
  useBranchView,
} from '../../lib/store'
import { prefersReduced, timelineLive, vEnter } from '../../lib/motion'
import { onReveal, useVirtualEntries } from '../../lib/virtualEntries'
import { focusTarget } from '../../lib/state/ui'
import { pendingPlace, rememberPlace } from '../../lib/state/scrollPlace'
import type { TimelineEntry } from '../../lib/store'
import type { Node } from '../../lib/types'
import UserRow from './UserRow.vue'
import AssistantGroup from './AssistantGroup.vue'
import ContextPills from './ContextPills.vue'
import CompactionRow from './CompactionRow.vue'
import ErrorRow from './ErrorRow.vue'
import TimelineRail from './TimelineRail.vue'

const props = defineProps<{
  /** The shell's composer, where the rail stops. */
  composerOf: () => HTMLElement | null
}>()

const view = useBranchView()
const { timeline, busy: branchBusy, owed: owedReply } = view
/** The branch runs a turn, and its end is what this view shows (the main
 *  line shown short of its end shows nothing live). */
const isBusy = computed(() => branchBusy.value && (view.sub || !hasLater.value))

/**
 * Render keys. A group is keyed by the entry before it, not by its nodes:
 * those change all through a turn (stream → assistant → tool results → …),
 * and a changing key would remount the group mid-turn — replaying its
 * entrance and dropping its open reasoning / expanded tools. Two groups are
 * never adjacent (a group only ends at a user message), so this is unique.
 * A turn whose window began mid-turn keeps that start's key instead as the
 * rest of it loads above (`state.seams`).
 */
const entries = computed(() => {
  let prev = 'root'
  return timeline.value.map((entry) => {
    // A compaction's line is keyed by where it stands, so the one being
    // written gives way to the written one in place, without an entrance.
    const key =
      entry.kind === 'compaction' || entry.kind === 'compacting'
        ? `cline:${prev}`
        : entry.kind !== 'group'
          ? dataId(entry)
          : entry.seam
            ? `group@${entry.seam}`
            : `group:${prev}`
    prev = key
    return { entry, key }
  })
})

const scrollEl = ref<HTMLElement | null>(null)
const innerEl = ref<HTMLElement | null>(null)
const railEl = ref<InstanceType<typeof TimelineRail> | null>(null)

/** Roughly how much an entry renders (nodes). */
function weightOf(entry: TimelineEntry): number {
  return entry.kind === 'group' ? entry.items.length + entry.assistants.length : 1
}

/** Which entries render their content (the rest hold their measured place). */
const vt = useVirtualEntries({
  scrollEl,
  columnEl: innerEl,
  entries: () =>
    entries.value.map(({ entry, key }) => ({
      key,
      virtual: entry.kind === 'group',
      weight: weightOf(entry),
    })),
  forced: () => liveKeys(),
  measured: () => loadMore(true),
  quietArrivals: () => laterLanded(),
  bootAt: () => entries.value[placeIndex()]?.key ?? null,
})

// ---- the main line's earlier part ----
// A session opens on the end of its main line. What came before loads in
// the background up to BACKGROUND_NODES (the backfill has measured what is
// in first, so each step is one more stretch for it), and past that as a
// scroll nears the top. Either way it arrives above the view as blanks the
// backfill measures, so nothing in view moves.

/** Up to this many nodes of the main line load without a scroll asking. */
const BACKGROUND_NODES = 3000
/** A scroll this many screens from the top loads more. */
const EARLIER_SCREENS = 3

/** Where the shown stretch of the line ended at the last look: a change
 *  means a later stretch landed (or the end was reached), and what it
 *  brought below is history. */
let seenLineEnd = state.lineEnd
function laterLanded(): boolean {
  const landed = !view.sub && state.lineEnd !== seenLineEnd
  seenLineEnd = state.lineEnd
  return landed
}

function loadMore(background: boolean): void {
  if (view.sub || !hasEarlier.value) return
  if (background && pathNodes.value.length >= BACKGROUND_NODES) return
  void loadEarlier()
}
const nearBottom = ref(true)
/** Where the page ended (scrollHeight) at the last layoutTail. */
let laidEnd = 0
/** Where the page ended (scrollHeight) and the view was, at the last scroll
 *  event. */
let scrolledHeight = 0
let scrolledTop = 0

/** The rail reads the scroller / column through getters: a template ref
 *  passed straight through a prop (or read as one) trips vue-tsc's
 *  HTMLElement unwrap, hence the cast. */
function scrollRoot(): HTMLElement | null {
  return scrollEl.value as unknown as HTMLElement | null
}
function columnRoot(): HTMLElement | null {
  return innerEl.value as unknown as HTMLElement | null
}

/** The thread shows while you scroll, and fades out a moment after. */
const scrolling = ref(false)
let scrollTimer: ReturnType<typeof setTimeout> | undefined
/** Our own jumps to the end (new tokens, view switch) don't count as scrolling. */
let quietUntil = 0

function onScroll(): void {
  const el = scrollEl.value
  if (!el) return
  // At the end as it stands, or as last laid out: content that grows before
  // layoutTail runs again (the scroll event of our own pin can fire after it)
  // is not you scrolling away from the end.
  const end = laidEnd ? Math.min(el.scrollHeight, laidEnd) : el.scrollHeight
  // Nor does a scroll the content made take you to the end: a stretch
  // landing above moves the view by its height (scroll anchoring), and a
  // shrink below clamps it to the bottom — a leap with the page's height
  // changed is not yours. (A swipe moves a frame's worth at a time, and a
  // reply streaming in grows the page a little.)
  const moved =
    el.scrollHeight !== scrolledHeight && Math.abs(el.scrollTop - scrolledTop) > el.clientHeight / 2
  scrolledHeight = el.scrollHeight
  scrolledTop = el.scrollTop
  // short of the line's end, the bottom of what is loaded does not pin
  const atEnd = end - el.scrollTop - el.clientHeight < 90 && (view.sub || !hasLater.value)
  if (nearBottom.value || !moved) nearBottom.value = atEnd
  // nearing the top of what is loaded: load what came before; nearing its
  // bottom short of the line's end, what comes after
  if (el.scrollTop < el.clientHeight * EARLIER_SCREENS) loadMore(false)
  if (
    !view.sub &&
    hasLater.value &&
    el.scrollHeight - el.scrollTop - el.clientHeight < el.clientHeight * EARLIER_SCREENS
  )
    void loadLater()
  // Chromium does not anchor a scroller at offset 0: what lands above
  // would push the view down. A px down it holds still.
  if (el.scrollTop < 1 && !view.sub && hasEarlier.value) el.scrollTop = 1
  clearTimeout(placeTimer)
  placeTimer = setTimeout(savePlace, PLACE_DEBOUNCE)
  if (performance.now() < quietUntil) return
  scrolling.value = true
  clearTimeout(scrollTimer)
  scrollTimer = setTimeout(() => (scrolling.value = false), 1200)
}

/** Jump to the tail of the conversation. `smooth` eases there instead — used
 *  only to settle a just-sent prompt; following tokens must stay instant, or
 *  every token would restart the animation. */
function scrollToEnd(smooth = false): void {
  const el = scrollEl.value
  if (!el) return
  quietUntil = performance.now() + (smooth ? 600 : 150)
  if (smooth && !prefersReduced()) {
    el.scrollTo({ top: endTop(el), behavior: 'smooth' })
    return
  }
  el.scrollTop = endTop(el)
}

/**
 * The scroll offset that shows the end of the page as laid out. Not
 * scrollHeight: that also counts entries still rising into place (their
 * entrance transforms reach below the page), and the moment they land that
 * overflow is gone and the view drops by as much — the whole column jumps.
 * The spacer is the scroller's last child and never moves on its own.
 */
function endTop(el: HTMLElement): number {
  const spacer = spacerEl.value
  if (!spacer) return el.scrollHeight
  const pad = parseFloat(getComputedStyle(el).paddingBottom) || 0
  const bottom = spacer.getBoundingClientRect().bottom - el.getBoundingClientRect().top - el.clientTop
  return Math.max(0, Math.floor(el.scrollTop + bottom + pad - el.clientHeight))
}

onBeforeUnmount(() => clearTimeout(scrollTimer))

// ---- where you are, across a reload ----
// A view read away from its end keeps its place (the entry at the top and
// how far its top sat from the view's) per tab; the first open after a
// reload lands there (`pendingPlace`), the tree reaching back to it.

/** A scroll at rest this long records the place. */
const PLACE_DEBOUNCE = 300
let placeTimer: ReturnType<typeof setTimeout> | undefined

function savePlace(): void {
  clearTimeout(placeTimer)
  const sc = scrollEl.value
  const inner = innerEl.value
  const session = state.sessionId
  // a place still to land is not overwritten by the scrolls on the way
  if (view.sub || !session || !sc || !inner || pendingPlace.value) return
  if (nearBottom.value) return rememberPlace(session, null)
  // the first entry reaching below the view's top
  const top = sc.scrollTop - inner.offsetTop
  const wraps = inner.querySelectorAll<HTMLElement>(':scope > [data-entry]')
  let lo = 0
  let hi = wraps.length - 1
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    const w = wraps[mid]!
    if (w.offsetTop + w.offsetHeight > top) hi = mid
    else lo = mid + 1
  }
  const el = wraps[lo]
  const node = el?.dataset.nodeId
  if (!el || !node) return
  rememberPlace(session, { node, offset: inner.offsetTop + el.offsetTop - sc.scrollTop })
}

onMounted(() => window.addEventListener('pagehide', savePlace))
onBeforeUnmount(() => {
  window.removeEventListener('pagehide', savePlace)
  clearTimeout(placeTimer)
})

/** This view opened at a remembered place: it fades in without rising. A
 *  transform on the column is a change Chromium's scroll anchoring holds
 *  against everything in it, until the next scroll — and nothing scrolls
 *  there before the earlier part lands above (it pushed the view down a
 *  whole stretch). An open at the end is pinned there instead. */
const openedAtPlace = ref(false)

/** The entry the open in progress lands on instead of the end, -1 for none
 *  (a linked message wins). */
function placeIndex(): number {
  const p = pendingPlace.value
  if (!p || view.sub || p.session !== state.sessionId || focusTarget.value) return -1
  return entries.value.findIndex(({ entry }) => dataId(entry) === p.node)
}

/** Put the remembered place where it was in the view; false when its entry
 *  is not there. */
function landPlace(sc: HTMLElement): boolean {
  const p = pendingPlace.value
  const i = placeIndex()
  const inner = innerEl.value
  const el = i < 0 ? null : inner?.querySelector<HTMLElement>(`[data-entry="${i}"]`)
  if (!p || !inner || !el) return false
  quietUntil = performance.now() + 150
  sc.scrollTop = inner.offsetTop + el.offsetTop - p.offset
  nearBottom.value = false
  return true
}

/** Bumped by every "open at the end" request; only the newest one lands. */
let endSeq = 0
/** The request a mounted scroller has taken up: each lands once, however
 *  many times it is asked for (the scroller mounting asks again). */
let settled = 0

/**
 * Jump to the tail of a freshly opened view. The scroller may not exist yet
 * (opening from the landing page, or a cold boot on `/s/<id>` that shows the
 * booting mark, mounts it only after that pane's leave transition), so a
 * request that arrives too early waits for the ref instead of giving up.
 */
function requestEndScroll(): void {
  const seq = ++endSeq
  nearBottom.value = placeIndex() < 0
  void nextTick(() => settleEnd(seq))
}

function settleEnd(seq: number): void {
  if (seq !== endSeq) return
  const el = scrollEl.value
  if (!el) return // the scrollEl watcher below asks again once it mounts
  if (settled === seq) return
  settled = seq
  // Two frames: the first layout pass (and the sheet's entrance) finish before
  // the end is trustworthy.
  requestAnimationFrame(() => {
    if (seq !== endSeq) return
    const node = scrollEl.value
    if (!node) return
    // a reload lands where it left off, held a frame on (the first layout
    // pass may still move it)
    if (landPlace(node)) {
      requestAnimationFrame(() => {
        if (seq === endSeq && scrollEl.value === node) landPlace(node)
        pendingPlace.value = null
      })
      return
    }
    // (a sub-agent's panel settling first leaves the chat's place alone)
    if (!view.sub) pendingPlace.value = null
    quietUntil = performance.now() + 150
    node.scrollTop = endTop(node)
    nearBottom.value = true
    requestAnimationFrame(() => {
      if (seq !== endSeq || scrollEl.value !== node) return
      // A linked message lands in place of the end (and the end stops
      // pulling at once, not on the scroll event a frame later).
      if (focusIndex >= 0) {
        jumpTo(focusIndex, false)
        focusIndex = -1
        nearBottom.value = false
        return
      }
      quietUntil = performance.now() + 150
      node.scrollTop = endTop(node)
      nearBottom.value = true
    })
  })
}

// The scroller mounts a tick (and a transition) after the session opens:
// retry the pending end scroll then.
watch(scrollEl, (el) => {
  if (el && endSeq) settleEnd(endSeq)
})

/** The entry a linked message opens at, waiting for the end scroll's turn. */
let focusIndex = -1

/** The entry holding the node `prefix` names; -1 when none does. */
function entryOf(prefix: string): number {
  return timeline.value.findIndex((e) => {
    const nodes =
      e.kind === 'group'
        ? e.assistants
        : e.kind === 'context'
          ? e.nodes
          : e.kind === 'compacting'
            ? []
            : [e.node]
    return nodes.some((n) => n.id.startsWith(prefix))
  })
}

// Code about to look a node up in the DOM (a side question's "show me")
// renders its entry first.
const offReveal = onReveal((node) => {
  const i = entryOf(node)
  const key = i < 0 ? undefined : entries.value[i]?.key
  if (key === undefined) return false
  vt.pin(key)
  return true
})
onBeforeUnmount(offReveal)

// A linked message (`focusTarget`) opens once its session shows it, in
// place of the end a fresh open lands on.
watch(
  () => [focusTarget.value, timeline.value] as const,
  ([focus]) => {
    if (view.sub || !focus || !state.sessionId?.startsWith(focus.session)) return
    const i = entryOf(focus.node)
    if (i < 0) {
      // maybe in the part of the main line not loaded yet
      if (hasEarlier.value) void ensureFullPath()
      return
    }
    focusTarget.value = null
    focusIndex = i
    nearBottom.value = false
    const seq = ++endSeq
    void nextTick(() => settleEnd(seq))
  },
  // The view mounts after a cold boot's session has loaded.
  { immediate: true },
)

// ---- the page below the timeline ----

/** Blank kept under the reply while a turn runs, px. */
const LIVE_ROOM = 120
/** Where the page ends, in column px: the column plus the spacer reach it. */
let floor = 0
/** A prompt was just sent and the view has not eased to it yet. */
let sentPending = false
/** The smooth settle onto a just-sent prompt is still running: following
 *  the end must not cut it short. */
let settlingUntil = 0
const spacerEl = ref<HTMLElement | null>(null)
/** The scroller's height at the last layout. */
let viewHeight = 0
/** Blank the scroller took back when it shrank (the composer growing — a
 *  decision unfolding, the tasks panel), owed back when it grows again. */
let lent = 0
/** The turn just ended: the blank it held eases away instead of snapping. */
let turnEnded = false
const SETTLE_MS = 260

/** The newest prompt on the path — the one a just-started turn answers. */
const lastPromptId = computed(() => {
  for (let i = timeline.value.length - 1; i >= 0; i--) {
    const e = timeline.value[i]
    if (e.kind === 'user') return e.node.id
  }
  return ''
})

/** The prompt most recently rendered; only a change means something was sent. */
let seenPrompt = ''

watch(
  lastPromptId,
  (id) => {
    // A new prompt while a turn starts is one you just sent. (History loads
    // change this too, but they reset the view below, and no turn is
    // starting.)
    const sent = !!id && id !== seenPrompt && isBusy.value
    seenPrompt = id
    if (!sent) return
    sentPending = true
    nearBottom.value = true
  },
  { flush: 'pre' },
)

/** Set the floor, size the spacer to reach it, and keep the end pinned while
 *  following. */
function layoutTail(): void {
  const sc = scrollEl.value
  const inner = innerEl.value
  const spacer = spacerEl.value
  if (!sc || !inner || !spacer) return
  vt.resized(inner.clientWidth)
  const height = inner.offsetHeight
  const shown = spacer.offsetHeight
  const sent = sentPending && isBusy.value
  sentPending = false
  if (!isBusy.value) {
    // Between turns: just the column.
    floor = height
    lent = 0
  } else {
    // A turn held up on a decision writes nothing until it is answered.
    const live = !state.decisions.some((d) => d.session === state.sessionId)
    floor = Math.max(floor, height + (live ? LIVE_ROOM : 0))
    // While you follow the end, a resize of the scroller comes out of the
    // blank first, so the page under you stays put instead of riding up.
    const delta = sc.clientHeight - viewHeight
    if (viewHeight && delta && nearBottom.value && !sent) {
      const shift = delta < 0 ? -Math.min(-delta, floor - height) : Math.min(delta, lent)
      floor += shift
      lent -= shift
    }
    // Following the end, a shrink above it doesn't leave its blank behind;
    // otherwise the blank only has to reach the bottom of the view.
    const cs = getComputedStyle(sc)
    const view = sc.scrollTop + sc.clientHeight - parseFloat(cs.paddingTop) - parseFloat(cs.paddingBottom)
    floor = Math.min(floor, Math.max(height + LIVE_ROOM, nearBottom.value ? 0 : view))
  }
  viewHeight = sc.clientHeight
  const tail = floor - height
  spacer.style.height = `${tail}px`
  laidEnd = sc.scrollHeight
  if (turnEnded && tail < shown && !prefersReduced()) {
    quietUntil = performance.now() + SETTLE_MS
    spacer
      .animate([{ height: `${shown}px` }, { height: `${tail}px` }], {
        duration: SETTLE_MS,
        easing: 'cubic-bezier(0.22, 1, 0.36, 1)',
      })
      .finished.then(
        () => railEl.value?.measure(),
        () => {},
      )
  }
  turnEnded = false
  // An earlier stretch landing above during the smooth settle onto a sent
  // prompt: Chromium does not anchor mid smooth scroll, so the view would
  // stay where the content moved from. The prompt is at the end: go there.
  const first = entries.value[0]?.key
  const landedAbove = first !== laidFirst
  laidFirst = first
  if (sent) {
    settlingUntil = performance.now() + 600
    scrollToEnd(true)
  } else if (landedAbove && performance.now() < settlingUntil) {
    settlingUntil = 0
    scrollToEnd()
  } else if (nearBottom.value && performance.now() >= settlingUntil) {
    scrollToEnd()
  }
  // a streaming turn resizes the column every few frames: the ticks follow
  // at once, else each resize would restart their glide and keep it running
  railEl.value?.measure(!isBusy.value)
}

/** The first entry at the last layoutTail. */
let laidFirst: string | undefined

// A finished turn gives back the blank it held, whether or not its last
// render resized anything.
watch(isBusy, (busy) => {
  if (busy) return
  turnEnded = true
  void nextTick(layoutTail)
})

// One observer per mounted timeline (the column remounts per session): the
// column's size covers every growth and shrink, the scroller's a composer
// that grows or shrinks under it.
watch(
  [scrollEl, innerEl],
  ([sc, inner], _, onCleanup) => {
    if (!sc || !inner) return
    const ro = new ResizeObserver(layoutTail)
    ro.observe(sc)
    ro.observe(inner)
    onCleanup(() => ro.disconnect())
  },
  { flush: 'post' },
)

// A different view renders all at once: that render is "history" (no entrance
// motion), a pending send settle is dropped, and this path's prompt counts as
// already seen. Anything mounted afterwards is live.
function resetView(): void {
  timelineLive.value = false
  vt.reset()
  vt.sync()
  sentPending = false
  floor = 0
  lent = 0
  laidEnd = 0
  turnEnded = false
  seenPrompt = lastPromptId.value
  void nextTick(() => {
    timelineLive.value = true
    // A same-height swap fires no resize; the spacer still starts over.
    layoutTail()
  })
}

// The tree leaves a sub-agent's nodes out until it is shown (and a replaced
// tree drops them again).
watchEffect(() => {
  if (view.sub && view.branch.value) void ensureAgentNodes(view.branch.value)
})

// A session switch remounts the timeline column (keyed by session and branch
// below): the sheet fades in as one and opens on its latest message. The end
// scroll waits for the scroller if this view is still mounting (landing →
// session), so a session always opens at the end. Another sub-agent in the
// panel is the same: another conversation. A variant switch (the inline
// switcher) only moves the head: the path before it stays mounted and the
// swapped tail renders in place.
// Back at the line's end from a stretch short of it (a prompt sent from
// there): it opens there as a session does.
watch(
  () => state.endJumps,
  () => {
    if (view.sub) return
    seenLineEnd = state.lineEnd
    resetView()
    requestEndScroll()
  },
)

// Shown from elsewhere instead (the rail's jumps to the top or to a turn):
// it opens at that prompt, or the first entry, the way a reload opens at
// its place.
watch(
  () => state.lineJump.seq,
  () => {
    if (view.sub) return
    seenLineEnd = state.lineEnd
    // the place first: the virtual list boots from it
    const want = state.lineJump.node
    const at = want ? entries.value.find(({ entry }) => dataId(entry) === want) : entries.value[0]
    const session = state.sessionId
    if (at && session) {
      // a prompt lands where a jump to it does (its scroll margin)
      const offset = want ? PROMPT_MARGIN : (innerEl.value?.offsetTop ?? 0)
      pendingPlace.value = { session, node: dataId(at.entry), offset }
      openedAtPlace.value = true
    }
    resetView()
    requestEndScroll()
  },
)

/** A prompt jumped to sits this far below the view's top (scroll-mt-3). */
const PROMPT_MARGIN = 12

watch(
  () => [state.sessionId, view.branch.value],
  () => {
    clearTimeout(placeTimer)
    openedAtPlace.value = placeIndex() >= 0
    resetView()
    requestEndScroll()
  },
  { flush: 'pre', immediate: true },
)

/** Node id for data-node-id (entry keys). */
function dataId(e: TimelineEntry): string {
  if (e.kind === 'user') return e.node.id
  if (e.kind === 'context') return e.nodes[0]?.id ?? ''
  if (e.kind === 'compaction') return e.node.id
  if (e.kind === 'compacting') return 'compacting'
  return e.first ?? ''
}

/** While a turn runs, everything from its prompt on keeps rendering. */
function liveKeys(): string[] {
  if (!isBusy.value) return []
  const list = entries.value
  const at = list.findLastIndex((e) => e.entry.kind === 'user')
  return list.slice(Math.max(0, at)).map((e) => e.key)
}

watch(entries, () => vt.sync())
watch(isBusy, () => vt.update())
watch(
  scrollEl,
  (el) => {
    if (!el) return
    vt.observer()
    void vt.fill()
  },
  { flush: 'post' },
)
onBeforeUnmount(() => vt.dispose())

/** Only the last group can be the one streaming. */
const lastGroup = computed(() => timeline.value.findLastIndex((e) => e.kind === 'group'))

/** The user prompt a group answers: the nearest user entry before it. */
function promptOf(i: number): Node | null {
  for (let j = i - 1; j >= 0; j--) {
    const e = timeline.value[j]
    if (e.kind === 'user') return e.node
  }
  return null
}

/** The rail's fixed jumps: to the very top of the line, loading its start
 *  when that is not here, and to its very end. */
function jumpToStart(): void {
  if (!view.sub && hasEarlier.value) return void goToStart()
  const el = scrollEl.value
  el?.scrollTo({ top: 0, behavior: prefersReduced() ? 'auto' : 'smooth' })
}

function jumpToEnd(): void {
  if (!view.sub && hasLater.value) return void goToEnd()
  scrollToEnd(true)
}

/** The rail's jump to turn `n` of the main line: in place when its prompt
 *  is here, else loaded there. */
function jumpToTurn(n: number): void {
  if (view.sub) return
  const i = entries.value.findIndex(
    ({ entry }) => entry.kind === 'user' && turnNumbers.value.get(entry.node.id) === n,
  )
  if (i >= 0) return jumpTo(i)
  void goToTurn(n)
}

/** Jump to where an entry starts; a quiet flash marks where it landed. */
function jumpTo(i: number, smooth = true): void {
  const el = innerEl.value?.querySelector<HTMLElement>(`[data-entry="${i}"]`)
  if (!el) return
  el.scrollIntoView({ behavior: smooth && !prefersReduced() ? 'smooth' : 'auto', block: 'start' })
  el.classList.remove('flash-nav')
  void el.offsetWidth
  el.classList.add('flash-nav')
  setTimeout(() => el.classList.remove('flash-nav'), 1200)
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div ref="scrollEl" class="tl-scroll scroll-fade flex-1 overflow-y-auto pt-4 pb-2" @scroll="onScroll">
      <div
        :key="`${state.sessionId ?? ''}:${view.branch.value ?? ''}`"
        ref="innerEl"
        class="tl-col relative mx-auto flex max-w-[var(--col-max)] flex-col gap-5"
        :class="openedAtPlace ? 'enter-fade' : 'view-in'"
      >
        <template v-if="timeline.length">
          <!-- the main line goes back further: it loads as the view nears
               here. Never the scroll anchor (what lands below it must not
               push the view down), and a fixed height, so its leaving moves
               a whole px. -->
          <div
            v-if="!view.sub && hasEarlier"
            class="h-8 text-center text-meta leading-8 text-ink-3 [overflow-anchor:none]"
          >
            {{ t('chat.timeline.earlier') }}
          </div>
          <div
            v-for="({ entry, key }, i) in entries"
            :key="key"
            :ref="vt.wrapRef(key)"
            v-enter="vt.enters(key) ? (entry.kind === 'user' ? 'enter-send' : 'enter-rise') : false"
            :data-node-id="dataId(entry)"
            :data-entry="i"
            class="min-w-0 scroll-mt-3"
            :class="{ 'tl-quiet': vt.quiet.value.has(key) }"
            @click="vt.pin(key)"
            @focusin="vt.pin(key)"
          >
            <!-- flow-root: the measured box holds its children's margins -->
            <div v-if="vt.shown.value.has(key)" :ref="vt.bodyRef(key)" class="flow-root">
              <UserRow v-if="entry.kind === 'user'" :node="entry.node" :context="entry.contextNodes" />
              <AssistantGroup
                v-else-if="entry.kind === 'group'"
                :group="entry"
                :prompt="promptOf(i)"
                :live="isBusy && i === lastGroup"
              />
              <ContextPills v-else-if="entry.kind === 'context'" :nodes="entry.nodes" />
              <CompactionRow v-else-if="entry.kind === 'compaction'" :node="entry.node" />
              <CompactionRow v-else-if="entry.kind === 'compacting'" :live="entry" />
            </div>
          </div>
        </template>
        <!-- the main line goes on past here: it loads as the view nears
             it (never the scroll anchor either) -->
        <div
          v-if="!view.sub && hasLater"
          class="h-8 text-center text-meta leading-8 text-ink-3 [overflow-anchor:none]"
        >
          {{ t('chat.timeline.later') }}
        </div>
        <!-- the reply this branch still owes: its last exchange failed
             (or was never finished) — offer to run it again -->
        <ErrorRow v-if="owedReply" :owed="owedReply" />
      </div>
      <!-- the page below the timeline (layoutTail): room for the
           reply to grow into, and what folds give back -->
      <div ref="spacerEl" aria-hidden="true" />
    </div>
  </div>
  <!-- the thread rail down the right edge: a pinned minimap of the
       whole conversation (it stops above the composer where they meet) -->
  <TimelineRail
    ref="railEl"
    :scroll-of="scrollRoot"
    :column-of="columnRoot"
    :composer-of="props.composerOf"
    :earlier="!view.sub && hasEarlier"
    :later="!view.sub && hasLater"
    :reveal="isBusy || scrolling"
    @jump="jumpTo"
    @top="jumpToStart"
    @bottom="jumpToEnd"
    @turn="jumpToTurn"
  />
</template>
