<script setup lang="ts">
// Center column, the one paper sheet on the canvas: header, the grouped
// timeline, and the composer card. Entries come from the store's `timeline`
// computed (user / context pills / assistant group); the content
// column is capped at --col-max (main.css), centered.
//
// A prompt you just sent opens near the top of the sheet, with the rest of
// the page left blank: the reply grows down into that blank rather than
// pinning your eye to the sliver just above the composer. The blank is a
// tail spacer after the timeline whose height shrinks as the reply fills it;
// once it is used up the scroller follows the tail as usual. Only a prompt
// that arrives while a turn is starting (one you just sent) anchors — a
// session switch still lands at the end of the conversation, and a branch
// switch keeps your place in it.
//
// The thread is the rail down the sheet's right edge (TimelineRail), which
// owns its measurement and drawing. This file owns the scroll state and
// jumping.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { activeDelta, isBusy, owedReply, queuedPrompts, state, timeline } from '../../lib/store'
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
 * (opening from the landing page mounts it only after that pane's leave
 * transition, and a cold boot renders the landing first), so a request that
 * arrives too early waits for the ref instead of giving up.
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

// ---- a just-sent prompt opens on a blank page ----

/** Distance from the scroller's top edge to the freshly sent prompt. */
const ANCHOR_GAP = 28
/** The prompt the blank page hangs from (a user node id), if any. */
const anchorNode = ref<string | null>(null)
/** Height of the blank page below the timeline, px. */
const tailSpace = ref(0)

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
/** The blank page for the current anchor has already been scrolled into place. */
let anchorPlaced = false

watch(
  lastPromptId,
  (id) => {
    // A new prompt while a turn starts is one you just sent. (History loads
    // change this too, but they reset the anchor in the view watcher below,
    // and no turn is starting.)
    const sent = !!id && id !== seenPrompt && isBusy.value
    seenPrompt = id
    if (!sent) return
    anchorNode.value = id
    anchorPlaced = false
    nearBottom.value = true
  },
  { flush: 'pre' },
)

let padEl: HTMLElement | null = null
let padBottom = 0

/**
 * Size the blank page so the anchored prompt sits ANCHOR_GAP below the
 * scroller's top. offsetTop is layout: the entrance animations may still be
 * running, and transforms don't move it.
 */
function measureTail(): void {
  const sc = scrollRoot()
  const inner = columnRoot()
  const id = anchorNode.value
  const el = sc && inner && id ? inner.querySelector<HTMLElement>(`[data-node-id="${CSS.escape(id)}"]`) : null
  if (!sc || !inner || !el) {
    tailSpace.value = 0
    return
  }
  if (sc !== padEl) {
    padEl = sc
    padBottom = parseFloat(getComputedStyle(sc).paddingBottom) || 0
  }
  const top = inner.offsetTop + el.offsetTop
  const bottom = inner.offsetTop + inner.offsetHeight
  tailSpace.value = Math.max(0, sc.clientHeight - (bottom - top) - ANCHOR_GAP - padBottom)
}

watch(
  () => [
    timeline.value.length,
    activeDelta.value?.text.length ?? 0,
    activeDelta.value?.reasoning.length ?? 0,
  ],
  async () => {
    await nextTick()
    const tail = tailSpace.value
    measureTail()
    if (tailSpace.value !== tail) await nextTick()
    if (tailSpace.value > 0) {
      // While the blank is still open the scroll target stays put (the spacer
      // shrinks by exactly what the reply grows): settle there once, smoothly,
      // and otherwise leave the view alone.
      if (!anchorPlaced) {
        anchorPlaced = true
        scrollToEnd(true)
      }
    } else if (nearBottom.value) {
      scrollToEnd()
    }
  },
)

// A different view renders all at once: that render is "history" (no entrance
// motion), the blank-page anchor is dropped, and this path's prompt counts as
// already seen. Anything mounted afterwards is live.
function resetView(): void {
  timelineLive.value = false
  anchorNode.value = null
  anchorPlaced = false
  tailSpace.value = 0
  seenPrompt = lastPromptId.value
  void nextTick(() => {
    timelineLive.value = true
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

// A branch switch (the branch panel, the inline variant switcher) keeps the
// sheet and your place in it: the path before the fork is not remounted, and
// the swapped tail renders as history — an instant swap, no fade, no jump.
watch(() => state.activeBranch, resetView, { flush: 'pre' })

// Context panel / drawer → scroll to a node and flash it. Pills may carry the
// id in data-node-ids (grouped), so fall back to a contains-match.
watch(
  () => state.focus,
  async (f) => {
    if (!f) return
    await nextTick()
    const root = scrollEl.value
    if (!root) return
    let el = root.querySelector(`[data-node-id="${f.nodeId}"]`)
    if (!el) {
      el =
        [...root.querySelectorAll('[data-node-ids]')].find((x) =>
          (x.getAttribute('data-node-ids') ?? '').split(' ').includes(f.nodeId),
        ) ?? null
    }
    if (!el) return
    el.scrollIntoView({ behavior: prefersReduced() ? 'auto' : 'smooth', block: 'center' })
    el.classList.add('flash')
    setTimeout(() => el.classList.remove('flash'), 2000)
  },
)

/** Node id for data-node-id (focus targets). */
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
      <SessionBar v-if="state.sessionId" part="branch" />
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
        <SessionBar v-if="state.sessionId" part="tools" />
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
        v-if="!state.sessionId"
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
            <!-- the page still to be written: keeps the just-sent prompt at
                 the top while its reply grows down into the blank -->
            <div v-if="tailSpace > 0" aria-hidden="true" :style="{ height: `${tailSpace}px` }" />
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
