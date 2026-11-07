<script setup lang="ts">
// Center column, the one paper sheet on the canvas: header, the grouped
// timeline, and the composer card. Entries come from the store's `timeline`
// computed (user / context pills / assistant group); the content
// column is capped at --col-max (main.css), centered. On phones and in
// portrait the sheet drops its card edges and becomes the page (main.css).
//
// The page never shrinks under you: a blank tail after the timeline (the
// spacer) takes back whatever the column gives up — a running call's output
// folding into its summary line, a fold closed — so what you are reading
// stays put, and growth fills that blank before it moves the page. Its floor
// rises three ways: a prompt you just sent opens near the top of the sheet,
// the reply growing down into the blank below it rather than pinning your eye
// to the sliver above the composer; while a turn runs (and is not waiting on
// a decision), LIVE_ROOM stays free under the reply, so whatever shows up next
// (a tall preview, a tool's output) lands in view; otherwise the column's own
// height. Any size change
// of the column or the scroller — not only new tokens — re-pins the end while
// you follow it, after the blank has absorbed what it can: a composer that
// grows (a decision unfolding) eats into the blank, not the page, and gets it
// back when it shrinks. Only a prompt that arrives while a turn is starting
// (one you just sent) anchors — a session switch still lands at the end of the
// conversation, and a branch switch keeps your place in it.
//
// The thread is the rail down the sheet's right edge (TimelineRail), which
// owns its measurement and drawing. This file owns the scroll state and
// jumping.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { connected, isBusy, owedReply, queuedPrompts, state, timeline } from '../../lib/store'
import { prefersReduced, timelineLive, vEnter } from '../../lib/motion'
import { layout, toggleSidebar } from '../../lib/layout'
import type { TimelineEntry } from '../../lib/store'
import type { Node } from '../../lib/types'
import UserRow from './UserRow.vue'
import AssistantGroup from './AssistantGroup.vue'
import ContextPills from './ContextPills.vue'
import CardDrawer from './CardDrawer.vue'
import Composer from './Composer.vue'
import ErrorRow from './ErrorRow.vue'
import TasksPanel from './TasksPanel.vue'
import TimelineRail from './TimelineRail.vue'
import Welcome from './Welcome.vue'
import SessionBar from './SessionBar.vue'
import RumuMark from '../brand/RumuMark.vue'
import Icon from '../ui/Icon.vue'

const scrollEl = ref<HTMLElement | null>(null)
const innerEl = ref<HTMLElement | null>(null)
const composerEl = ref<HTMLElement | null>(null)
const nearBottom = ref(true)

/** The rail reads the scroller / column / composer through getters: a template ref
 *  passed straight through a prop (or read as one) trips vue-tsc's
 *  HTMLElement unwrap, hence the cast. */
function scrollRoot(): HTMLElement | null {
  return scrollEl.value as unknown as HTMLElement | null
}
function columnRoot(): HTMLElement | null {
  return innerEl.value as unknown as HTMLElement | null
}
function composerRoot(): HTMLElement | null {
  return composerEl.value as unknown as HTMLElement | null
}

/** The thread shows while you scroll, and fades out a moment after. */
const scrolling = ref(false)
let scrollTimer: ReturnType<typeof setTimeout> | undefined
/** Our own jumps to the end (new tokens, view switch) don't count as scrolling. */
let quietUntil = 0

function onScroll(): void {
  const el = scrollEl.value
  if (!el) return
  nearBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 90
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
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' })
    return
  }
  el.scrollTop = el.scrollHeight
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
  // scrollHeight is trustworthy.
  requestAnimationFrame(() => {
    if (seq !== endSeq) return
    const node = scrollEl.value
    if (!node) return
    quietUntil = performance.now() + 150
    node.scrollTop = node.scrollHeight
    nearBottom.value = true
    requestAnimationFrame(() => {
      if (seq !== endSeq || scrollEl.value !== node) return
      quietUntil = performance.now() + 150
      node.scrollTop = node.scrollHeight
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

/** Distance from the scroller's top edge to the freshly sent prompt. */
const ANCHOR_GAP = 28
/** Blank kept under the reply while a turn runs, px. */
const LIVE_ROOM = 120
/** Where the page ends, in column px: the column plus the spacer reach it. */
let floor = 0
/** A just-sent prompt (node id) whose blank page is not laid out yet. */
let anchorPending: string | null = null
/** The smooth settle onto a just-sent prompt is still running: following
 *  the end must not cut it short. */
let settlingUntil = 0
const spacerEl = ref<HTMLElement | null>(null)
/** The scroller's height at the last layout. */
let viewHeight = 0
/** Blank the scroller took back when it shrank (the composer growing — a
 *  decision unfolding, the tasks panel), owed back when it grows again. */
let lent = 0

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
    anchorPending = id
    nearBottom.value = true
  },
  { flush: 'pre' },
)

/**
 * Raise the floor, size the spacer to reach it, and keep the end pinned
 * while following. offsetTop is layout: the entrance animations may still
 * be running, and transforms don't move it.
 */
function layoutTail(): void {
  const sc = scrollEl.value
  const inner = innerEl.value
  const spacer = spacerEl.value
  if (!sc || !inner || !spacer) return
  const height = inner.offsetHeight
  const prompt = anchorPending
    ? inner.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(anchorPending)}"]`)
    : null
  if (prompt) {
    // Enough page below the prompt for it to sit ANCHOR_GAP under the top.
    const pad = parseFloat(getComputedStyle(sc).paddingBottom) || 0
    floor = Math.max(floor, prompt.offsetTop + sc.clientHeight - ANCHOR_GAP - pad)
    anchorPending = null
  }
  // A turn held up on a decision writes nothing until it is answered.
  const live = isBusy.value && !state.decisions.some((d) => d.session === state.sessionId)
  floor = Math.max(floor, height + (live ? LIVE_ROOM : 0))
  // While you follow the end, a resize of the scroller comes out of the blank
  // first, so the page under you stays put instead of riding up with it.
  const delta = sc.clientHeight - viewHeight
  if (viewHeight && delta && nearBottom.value && !prompt) {
    const shift = delta < 0 ? -Math.min(-delta, floor - height) : Math.min(delta, lent)
    floor += shift
    lent -= shift
  }
  viewHeight = sc.clientHeight
  spacer.style.height = `${floor - height}px`
  if (prompt) {
    settlingUntil = performance.now() + 600
    scrollToEnd(true)
  } else if (nearBottom.value && performance.now() >= settlingUntil) {
    scrollToEnd()
  }
}

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
// motion), the blank-page anchor is dropped, and this path's prompt counts as
// already seen. Anything mounted afterwards is live.
function resetView(): void {
  timelineLive.value = false
  anchorPending = null
  floor = 0
  lent = 0
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

/** Node id for data-node-id (entry keys and the anchored-prompt measurement). */
function dataId(e: TimelineEntry): string {
  if (e.kind === 'user') return e.node.id
  if (e.kind === 'context') return e.nodes[0]?.id ?? ''
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

// ---- composer (Composer.vue) ----
</script>

<template>
  <section
    class="sheet m-2 flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface sm:m-3 lg:mx-0 lg:my-4"
  >
    <header
      class="flex min-h-16 flex-none items-center gap-2 px-4 sm:px-6"
      :class="{ 'border-b border-line-soft': state.sessionId }"
    >
      <!-- the sessions sidebar is a drawer below lg -->
      <button
        class="icon-btn -ml-1.5 size-8 flex-none lg:hidden"
        :title="t('chat.header.sessions')"
        :aria-label="t('chat.header.openSessions')"
        :aria-expanded="layout.sidebarOpen"
        @click="toggleSidebar"
      >
        <Icon name="menu" :size="16" />
      </button>
      <Transition
        mode="out-in"
        enter-active-class="transition duration-300 ease-soft"
        enter-from-class="translate-y-1 opacity-0"
        leave-active-class="transition duration-150"
        leave-to-class="opacity-0"
      >
        <span
          :key="state.sessionId ?? ''"
          class="min-w-0 truncate text-title font-bold"
          :title="state.session?.title"
        >
          {{ state.session?.title || (state.sessionId ? t('common.untitled') : '') }}
        </span>
      </Transition>
      <span class="ml-auto flex min-w-0 flex-none items-center gap-2">
        <Transition
          enter-active-class="transition duration-200 ease-soft"
          enter-from-class="translate-x-1 opacity-0"
          leave-active-class="transition duration-200"
          leave-to-class="opacity-0"
        >
          <span
            v-if="isBusy"
            class="mr-1 inline-flex items-center gap-2 text-micro font-semibold text-brand-strong"
          >
            <span class="dot-live size-1.5 rounded-full bg-brand" />
            <span class="hidden sm:inline">{{ t('chat.header.running') }}</span>
          </span>
        </Transition>
        <SessionBar v-if="state.sessionId" />
      </span>
    </header>

    <Transition
      mode="out-in"
      enter-active-class="transition duration-300 ease-soft"
      enter-from-class="translate-y-2 opacity-0"
      leave-active-class="transition duration-150 ease-soft"
      leave-to-class="-translate-y-1 opacity-0"
    >
      <!-- landing: the composer is the one thing to touch -->
      <div
        v-if="!state.sessionId && !state.booting"
        key="landing"
        class="flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-4 pb-10"
      >
        <div class="w-full max-w-[var(--col-max)]">
          <Welcome />
          <div class="mt-8">
            <Composer />
          </div>
        </div>
      </div>

      <!-- booting straight into a session (a refresh on `/s/<id>`): hold the
           sheet with a quietly breathing wordmark rather than flashing the
           landing page while the tree loads -->
      <div v-else-if="!state.sessionId" key="booting" class="flex min-h-0 flex-1 items-center justify-center">
        <RumuMark :height="30" live :offline="!connected" />
      </div>

      <!-- the conversation: timeline + composer at the foot of the sheet -->
      <div v-else key="chat" class="relative flex min-h-0 flex-1 flex-col">
        <div class="flex min-h-0 flex-1 flex-col">
          <div
            ref="scrollEl"
            class="tl-scroll scroll-fade flex-1 overflow-y-auto pt-4 pb-2"
            @scroll="onScroll"
          >
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

        <div ref="composerEl" class="px-3 pt-1 pb-3 sm:px-6 sm:pb-5">
          <!-- running background tasks of this branch + opened task output -->
          <TasksPanel />
          <!-- steering prompts accepted while busy, until they are written -->
          <div
            v-if="queuedPrompts.length"
            class="mx-auto mb-1.5 flex w-full max-w-[var(--col-max)] flex-col gap-1"
          >
            <div
              v-for="q in queuedPrompts"
              :key="q.id"
              class="ml-auto w-fit max-w-[85%] rounded-sheet rounded-br-xs border border-dashed border-brand/40 bg-brand-soft/50 px-4 py-2 text-message text-ink-3 sm:max-w-[70%]"
            >
              <span class="mr-1.5 text-micro font-semibold text-ink-2">{{ t('chat.header.queued') }}</span
              >{{ q.text }}
            </div>
          </div>
          <Composer />
        </div>
        <!-- the thread rail down the right edge: a pinned minimap of the
             whole conversation (it stops above the composer where they meet) -->
        <TimelineRail
          :scroll-of="scrollRoot"
          :column-of="columnRoot"
          :composer-of="composerRoot"
          :reveal="isBusy || scrolling"
          @jump="jumpTo"
        />
      </div>
    </Transition>

    <CardDrawer />
  </section>
</template>
