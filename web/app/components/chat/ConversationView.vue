<script setup lang="ts">
// The conversation view: the grouped timeline of the active path. Entries come from the store's `timeline`
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
// place.
//
// The thread is the rail down the sheet's right edge (TimelineRail), which
// owns its measurement and drawing. This file owns the scroll state and
// jumping; the composer below belongs to the shell (ChatView), which lends
// it to the rail through `composerOf`.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { isBusy, owedReply, state, timeline } from '../../lib/store'
import { prefersReduced, timelineLive, vEnter } from '../../lib/motion'
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

const scrollEl = ref<HTMLElement | null>(null)
const innerEl = ref<HTMLElement | null>(null)
const railEl = ref<InstanceType<typeof TimelineRail> | null>(null)
const nearBottom = ref(true)
/** Where the page ended (scrollHeight) at the last layoutTail. */
let laidEnd = 0

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
  nearBottom.value = end - el.scrollTop - el.clientHeight < 90
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

/** Bumped by every "open at the end" request; only the newest one lands. */
let endSeq = 0

/**
 * Jump to the tail of a freshly opened view. The scroller may not exist yet
 * (opening from the landing page, or a cold boot on `/s/<id>` that shows the
 * booting mark, mounts it only after that pane's leave transition), so a
 * request that arrives too early waits for the ref instead of giving up.
 */
function requestEndScroll(): void {
  const seq = ++endSeq
  nearBottom.value = true
  void nextTick(() => settleEnd(seq))
}

function settleEnd(seq: number): void {
  if (seq !== endSeq) return
  const el = scrollEl.value
  if (!el) return // the scrollEl watcher below asks again once it mounts
  // Two frames: the first layout pass (and the sheet's entrance) finish before
  // the end is trustworthy.
  requestAnimationFrame(() => {
    if (seq !== endSeq) return
    const node = scrollEl.value
    if (!node) return
    quietUntil = performance.now() + 150
    node.scrollTop = endTop(node)
    nearBottom.value = true
    requestAnimationFrame(() => {
      if (seq !== endSeq || scrollEl.value !== node) return
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
  if (sent) {
    settlingUntil = performance.now() + 600
    scrollToEnd(true)
  } else if (nearBottom.value && performance.now() >= settlingUntil) {
    scrollToEnd()
  }
  railEl.value?.measure()
}

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

// A session switch remounts the timeline column (keyed by session below): the
// sheet fades in as one and opens on its latest message. The end scroll waits
// for the scroller if this view is still mounting (landing → session), so a
// session always opens at the end.
watch(
  () => state.sessionId,
  () => {
    resetView()
    requestEndScroll()
  },
  { flush: 'pre', immediate: true },
)

// A variant switch (the inline switcher) keeps the sheet and your place in
// it: the path before the fork is not remounted, and the swapped tail renders
// as history — an instant swap, no fade, no jump.
watch(() => state.activeBranch, resetView, { flush: 'pre' })

/** Node id for data-node-id (entry keys). */
function dataId(e: TimelineEntry): string {
  if (e.kind === 'user') return e.node.id
  if (e.kind === 'context') return e.nodes[0]?.id ?? ''
  if (e.kind === 'compaction') return e.node.id
  return e.safeNode?.id ?? e.assistants.at(-1)?.id ?? ''
}

/**
 * Render keys. A group is keyed by the entry before it, not by its nodes:
 * those change all through a turn (stream → assistant → tool results → …),
 * and a changing key would remount the group mid-turn — replaying its
 * entrance and dropping its open reasoning / expanded tools. Two groups are
 * never adjacent (a group only ends at a user message), so this is unique.
 */
const entries = computed(() => {
  let prev = 'root'
  return timeline.value.map((entry) => {
    const key = entry.kind === 'group' ? `group:${prev}` : dataId(entry)
    prev = key
    return { entry, key }
  })
})

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

/** Jump to where an entry starts; a quiet flash marks where it landed. */
function jumpTo(i: number): void {
  const el = innerEl.value?.querySelector<HTMLElement>(`[data-entry="${i}"]`)
  if (!el) return
  el.scrollIntoView({ behavior: prefersReduced() ? 'auto' : 'smooth', block: 'start' })
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
        :key="state.sessionId ?? ''"
        ref="innerEl"
        class="tl-col view-in relative mx-auto flex max-w-[var(--col-max)] flex-col gap-5"
      >
        <template v-if="timeline.length">
          <div
            v-for="({ entry, key }, i) in entries"
            :key="key"
            v-enter="entry.kind === 'user' ? 'enter-send' : 'enter-rise'"
            :data-node-id="dataId(entry)"
            :data-entry="i"
            class="min-w-0 scroll-mt-3"
          >
            <UserRow v-if="entry.kind === 'user'" :node="entry.node" :context="entry.contextNodes" />
            <AssistantGroup
              v-else-if="entry.kind === 'group'"
              :group="entry"
              :prompt="promptOf(i)"
              :live="isBusy && i === lastGroup"
            />
            <ContextPills v-else-if="entry.kind === 'context'" :nodes="entry.nodes" />
            <CompactionRow v-else-if="entry.kind === 'compaction'" :node="entry.node" />
          </div>
        </template>
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
    :reveal="isBusy || scrolling"
    @jump="jumpTo"
  />
</template>
