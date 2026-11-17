<script setup lang="ts">
// The composer's context meter, beside the model chip: a small pie of how
// much of the next model's window the conversation fills. Grey; amber once
// it nears the auto-compaction threshold; while a turn runs, the part not
// measured yet (the reply streaming, results since the last call) shows as
// a coral slice ahead of the measured one. Hover (or focus, or a tap that
// pins it) opens a card with the numbers and where it compacts.
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue'
import type { CSSProperties, Ref } from 'vue'
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/vue'
import { t } from '../../i18n'
import { contextUse } from '../../lib/state/context'
import { compactBranch, showStats } from '../../lib/state/sessions'
import { state } from '../../lib/state/state'
import { fmtTokens } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const TIP_ID = `context-meter-${useId()}`

const use = computed(() => contextUse.value)
const busy = computed(() => !!state.activeBranch && !!state.busy[state.activeBranch])
/** Amber from a tenth short of where it compacts (of the window, with
 *  auto-compaction off). */
const warn = computed(() => {
  const u = use.value
  return !!u && u.used >= (u.threshold ?? u.window) * 0.9
})

// ---- the pie: shares eased toward the live numbers ----

const target = computed(() => {
  const u = use.value
  if (!u) return { measured: 0, live: 0 }
  const live = busy.value ? u.estimated : 0
  return { measured: Math.min((u.used - live) / u.window, 1), live: Math.min(live / u.window, 1) }
})
const shown = ref({ ...target.value })
let frame = 0

/** Settles without overshoot when growing; springs back when it shrinks
 *  (a compaction). */
const soft = (x: number) => 1 - (1 - x) ** 4
const spring = (x: number) => 1 + 2.2 * (x - 1) ** 3 + 1.2 * (x - 1) ** 2

watch(target, (to) => {
  cancelAnimationFrame(frame)
  const from = { ...shown.value }
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    shown.value = { ...to }
    return
  }
  const ease = to.measured + to.live < from.measured + from.live - 0.02 ? spring : soft
  const span = ease === spring ? 520 : 320
  const start = performance.now()
  const step = (now: number) => {
    const k = ease(Math.min((now - start) / span, 1))
    shown.value = {
      measured: Math.max(0, from.measured + (to.measured - from.measured) * k),
      live: Math.max(0, from.live + (to.live - from.live) * k),
    }
    if (now - start < span) frame = requestAnimationFrame(step)
  }
  frame = requestAnimationFrame(step)
})

/** A slice from share `a` to `b` of the turn, clockwise from 12 o'clock. */
function slice(a: number, b: number): string {
  const R = 4.4
  if (b - a <= 0.0005) return ''
  if (b - a >= 0.9995) return `M8 ${8 - R}A${R} ${R} 0 1 1 7.999 ${8 - R}Z`
  const pt = (s: number) => {
    const th = s * 2 * Math.PI - Math.PI / 2
    return `${(8 + R * Math.cos(th)).toFixed(3)} ${(8 + R * Math.sin(th)).toFixed(3)}`
  }
  return `M8 8L${pt(a)}A${R} ${R} 0 ${b - a > 0.5 ? 1 : 0} 1 ${pt(b)}Z`
}
const measuredPath = computed(() => slice(0, Math.min(shown.value.measured, 1)))
const livePath = computed(() => {
  const { measured, live } = shown.value
  return slice(Math.min(measured, 1), Math.min(measured + live, 1))
})

// ---- the card ----

const pct = (n: number) => `${Math.min((n / (use.value?.window || 1)) * 100, 100)}%`
const status = computed(() => {
  const u = use.value
  if (!u) return ''
  if (u.threshold === null) return t('chat.context.manual')
  if (u.used >= u.threshold) return t('chat.context.over')
  return t('chat.context.left', { n: fmtTokens(u.threshold - u.used) })
})

const pill = ref<HTMLElement | null>(null)
const tip = ref<HTMLElement | null>(null)
const hover = ref(false)
const pinned = ref(false)
const open = computed(() => !!use.value && (hover.value || pinned.value))

let openTimer: ReturnType<typeof setTimeout> | undefined
let closeTimer: ReturnType<typeof setTimeout> | undefined

function enter(): void {
  clearTimeout(closeTimer)
  if (hover.value) return
  clearTimeout(openTimer)
  openTimer = setTimeout(() => (hover.value = true), 300)
}
function enterNow(): void {
  clearTimeout(openTimer)
  clearTimeout(closeTimer)
  hover.value = true
}
function leave(): void {
  clearTimeout(openTimer)
  clearTimeout(closeTimer)
  closeTimer = setTimeout(() => (hover.value = false), 140)
}
function close(): void {
  clearTimeout(openTimer)
  hover.value = false
  pinned.value = false
}
function togglePin(): void {
  clearTimeout(openTimer)
  pinned.value = !pinned.value
}

function compactNow(): void {
  close()
  void compactBranch('')
}
function openStats(): void {
  close()
  showStats(null)
}

function onDocPointer(e: PointerEvent): void {
  const target = e.target as Node
  if (!pill.value?.contains(target) && !tip.value?.contains(target)) pinned.value = false
}
watch(pinned, (v) => {
  if (v) document.addEventListener('pointerdown', onDocPointer, true)
  else document.removeEventListener('pointerdown', onDocPointer, true)
})

onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  clearTimeout(openTimer)
  clearTimeout(closeTimer)
  document.removeEventListener('pointerdown', onDocPointer, true)
})

const { floatingStyles, isPositioned } = useFloating(
  // Template refs are narrowed by vue-tsc; floating-ui just reads them.
  pill as unknown as Readonly<Ref<HTMLElement | null>>,
  tip as unknown as Readonly<Ref<HTMLElement | null>>,
  {
    open,
    placement: 'top-start',
    strategy: 'fixed',
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip({ padding: 8 }), shift({ padding: 8 })],
  },
)

/** Hidden for the first frame of an open, so the card never flashes at 0,0. */
const ready = ref(false)
watch(isPositioned, (v) => {
  if (v) ready.value = true
})
watch(open, (v) => {
  if (v) ready.value = false
})
const tipStyle = computed<CSSProperties>(() => ({
  ...floatingStyles.value,
  visibility: ready.value ? 'visible' : 'hidden',
}))
</script>

<template>
  <span
    v-if="use"
    ref="pill"
    class="inline-flex flex-none"
    @mouseenter="enter"
    @mouseleave="leave"
    @focusin="enterNow"
    @focusout="leave"
    @keydown.escape="close"
  >
    <button
      type="button"
      class="inline-flex size-7 items-center justify-center rounded-full transition-colors duration-300 hover:bg-surface-2"
      :class="[warn ? 'text-warn' : 'text-ink-3', open && 'bg-surface-2']"
      :aria-label="t('chat.context.aria')"
      :aria-expanded="open"
      :aria-describedby="open ? TIP_ID : undefined"
      @click="togglePin"
    >
      <svg width="16" height="16" viewBox="0 0 16 16" class="flex-none" aria-hidden="true">
        <circle cx="8" cy="8" r="6.6" fill="none" stroke="currentColor" stroke-width="1.4" />
        <path v-if="livePath" :d="livePath" class="fill-brand opacity-70" />
        <path v-if="measuredPath" :d="measuredPath" fill="currentColor" />
      </svg>
    </button>

    <Teleport to="body">
      <Transition
        enter-active-class="transition-opacity duration-150 ease-soft"
        enter-from-class="opacity-0"
        leave-active-class="transition-opacity duration-100 ease-in"
        leave-to-class="opacity-0"
      >
        <div
          v-if="open && use"
          :id="TIP_ID"
          ref="tip"
          role="tooltip"
          :style="tipStyle"
          class="z-30 w-[17rem] max-w-[calc(100vw-2rem)] rounded-card border border-line-soft bg-surface px-3.5 pt-3 pb-2 shadow-pop"
          @mouseenter="enter"
          @mouseleave="leave"
        >
          <div class="flex items-baseline justify-between gap-3">
            <span class="text-meta font-semibold text-ink-3">{{ t('chat.context.label') }}</span>
            <span class="text-secondary font-semibold text-ink tabular-nums">
              {{ fmtTokens(use.used) }}
              <span class="font-normal text-ink-3">/ {{ fmtTokens(use.window) }}</span>
            </span>
          </div>
          <div class="relative mt-2.5 mb-1.5 h-1.5 rounded-full bg-surface-2">
            <div
              v-if="use.threshold !== null"
              class="context-reserve absolute inset-y-0 right-0 rounded-r-full"
              :style="{ left: pct(use.threshold) }"
            />
            <div
              class="absolute inset-y-0 left-0 rounded-full transition-[width] duration-300 ease-soft"
              :class="warn ? 'bg-warn' : 'bg-ink-3'"
              :style="{ width: pct(use.used) }"
            />
            <div
              v-if="use.threshold !== null"
              class="absolute -inset-y-[3px] w-0.5 rounded-full bg-ink-2"
              :style="{ left: `calc(${pct(use.threshold)} - 1px)` }"
            />
          </div>
          <p class="text-meta text-ink-2 tabular-nums">{{ status }}</p>
          <p v-if="use.assumed" class="text-micro text-ink-3">
            {{ t('chat.context.assumed', { n: fmtTokens(use.window) }) }}
          </p>
          <div class="-mx-1.5 mt-2 flex gap-0.5 border-t border-line-soft pt-1.5">
            <button
              type="button"
              class="inline-flex items-center gap-1.5 rounded-control px-2 py-1 text-meta text-ink-2 transition-colors hover:bg-surface-2 hover:text-ink disabled:pointer-events-none disabled:opacity-50"
              :disabled="busy"
              @click="compactNow"
            >
              <Icon name="history" :size="14" />{{ t('chat.context.compact') }}
            </button>
            <button
              type="button"
              class="inline-flex items-center gap-1.5 rounded-control px-2 py-1 text-meta text-ink-2 transition-colors hover:bg-surface-2 hover:text-ink"
              @click="openStats"
            >
              <Icon name="stats" :size="14" />{{ t('chat.context.stats') }}
            </button>
          </div>
        </div>
      </Transition>
    </Teleport>
  </span>
</template>
