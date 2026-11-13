<script setup lang="ts">
// Per-reply usage line under an assistant turn: "time · N calls · output".
// Hover or focus opens a card for the whole reply — its totals, where the
// time went, and every model call as a column (CallBars); a click opens the
// stats view at this reply. In a sub-agent's panel, which the stats view does
// not cover, a click pins the card instead.
//
// Hovering waits OPEN_DELAY before opening, so sweeping the pointer across the
// timeline does not flash the card; focus and tap open immediately.
//
// Placement is @floating-ui/vue (fixed strategy, flip + shift), teleported to
// <body> so the timeline scroller can never clip the card. The hover state
// has a short grace so the pointer can cross the gap into the card.
import { computed, onBeforeUnmount, ref, useId, watch } from 'vue'
import { t } from '../../i18n'
import type { CSSProperties, Ref } from 'vue'
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/vue'
import { showModel } from '../../lib/layout'
import { modelByRef } from '../../lib/state/models'
import { repliesOf, showStats, summarize, useBranchView } from '../../lib/store'
import type { TimelineGroup } from '../../lib/store'
import { fmtDuration, fmtTokens } from '../../lib/util'
import CallBars from '../stats/CallBars.vue'
import TimeSplit from '../stats/TimeSplit.vue'
import TokenChips from '../stats/TokenChips.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ group: TimelineGroup }>()
const view = useBranchView()

const TIP_ID = `usage-hud-${useId()}`

const pill = ref<HTMLElement | null>(null)
const tip = ref<HTMLElement | null>(null)
const hover = ref(false)
const pinned = ref(false)
const open = computed(() => hover.value || pinned.value)

/** Hover dwell before the card opens; keeps a pointer sweep from flashing it. */
const OPEN_DELAY = 300

let closeTimer: ReturnType<typeof setTimeout> | null = null
let openTimer: ReturnType<typeof setTimeout> | null = null

function cancelOpen(): void {
  if (openTimer !== null) {
    clearTimeout(openTimer)
    openTimer = null
  }
}

/** Pointer hover: arm the delay unless the card is already showing. */
function enter(): void {
  if (closeTimer !== null) {
    clearTimeout(closeTimer)
    closeTimer = null
  }
  if (hover.value || pinned.value || openTimer !== null) return
  openTimer = setTimeout(() => {
    openTimer = null
    hover.value = true
  }, OPEN_DELAY)
}

/** Focus: no dwell, since there is no accidental sweep to guard against. */
function enterNow(): void {
  cancelOpen()
  if (closeTimer !== null) {
    clearTimeout(closeTimer)
    closeTimer = null
  }
  hover.value = true
}

function leave(): void {
  cancelOpen()
  if (closeTimer !== null) clearTimeout(closeTimer)
  closeTimer = setTimeout(() => {
    closeTimer = null
    hover.value = false
  }, 140)
}

function close(): void {
  cancelOpen()
  hover.value = false
  pinned.value = false
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
  cancelOpen()
  if (closeTimer !== null) clearTimeout(closeTimer)
  document.removeEventListener('pointerdown', onDocPointer, true)
})

// ---- the reply ----

const reply = computed(() => {
  const first = props.group.assistants[0]
  if (!first) return null
  return repliesOf(view.nodes.value).find((r) => r.id === first.id) ?? null
})
const sum = computed(() => (reply.value ? summarize([reply.value]) : null))

/** No usage on the path (a fresh branch): drop any open/pinned state. */
watch(reply, (r) => {
  if (!r) close()
})

function onClick(): void {
  if (view.sub) {
    cancelOpen()
    pinned.value = !pinned.value
    return
  }
  const r = reply.value
  if (!r) return
  close()
  showStats({ from: r.id, to: r.id })
}

/** The reply's most used model, by catalog name (the id when no longer enabled). */
const model = computed(() => {
  const ref = sum.value?.models[0]?.ref ?? ''
  const info = modelByRef(ref)
  return { ref, name: info?.name ?? ref.slice(ref.indexOf('/') + 1), context: info?.context ?? null }
})

function openModel(): void {
  close()
  showModel(model.value.ref)
}

const whenText = computed(() => {
  const last = reply.value?.calls.at(-1)
  if (!last) return ''
  return new Date(last.node.at).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
})

const tiles = computed(() => {
  const s = sum.value
  if (!s) return []
  return [
    { label: t('usage.time'), value: fmtDuration(s.activeMs), unit: '' },
    { label: t('usage.output'), value: fmtTokens(s.output), unit: t('usage.unitTokens') },
    { label: t('usage.ttft'), value: s.avgTtftMs === null ? '—' : fmtDuration(s.avgTtftMs), unit: '' },
    {
      label: t('usage.hitRate'),
      value: s.hitRate === null ? '—' : `${Math.round(s.hitRate * 100)}%`,
      unit: '',
    },
  ]
})

/** What went wrong on the way, if anything. */
const troubles = computed(() => {
  const s = sum.value
  if (!s) return []
  const out: string[] = []
  if (s.retries) out.push(`${t('usage.retries')} ${s.retries}`)
  if (s.failures) out.push(t('usage.failures', { n: s.failures }))
  if (s.resets) out.push(t('usage.resets', { n: s.resets, count: s.resets }))
  return out
})

// ---- placement ----

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
  <div
    v-if="reply && sum"
    ref="pill"
    class="relative inline-flex"
    @mouseenter="enter"
    @mouseleave="leave"
    @focusin="enterNow"
    @focusout="leave"
    @keydown.escape="close"
  >
    <button
      type="button"
      class="-mx-1 inline-flex h-5 items-center gap-1.5 rounded-control px-1 text-micro tabular-nums text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink-2"
      :aria-label="t('usage.aria')"
      :aria-expanded="open"
      :aria-describedby="open ? TIP_ID : undefined"
      @click="onClick"
    >
      <span class="font-bold text-ink-2">{{ fmtDuration(sum.activeMs) }}</span>
      <template v-if="sum.calls > 1">
        <span aria-hidden="true" class="h-3 w-px bg-line" />
        <span>{{ t('usage.calls', { n: sum.calls, count: sum.calls }) }}</span>
      </template>
      <span aria-hidden="true" class="h-3 w-px bg-line" />
      <span>{{ t('usage.tokens', { n: fmtTokens(sum.output) }) }}</span>
      <span v-if="troubles.length" class="size-1.5 rounded-full bg-warn" aria-hidden="true" />
    </button>

    <Teleport to="body">
      <Transition
        enter-active-class="transition-opacity duration-150 ease-soft"
        enter-from-class="opacity-0"
        leave-active-class="transition-opacity duration-100 ease-in"
        leave-to-class="opacity-0"
      >
        <div
          v-if="open"
          :id="TIP_ID"
          ref="tip"
          role="tooltip"
          :style="tipStyle"
          class="z-30 w-[25rem] max-w-[calc(100vw-2rem)] rounded-card border border-line-soft bg-surface p-3 shadow-pop"
          @mouseenter="enter"
          @mouseleave="leave"
        >
          <!-- the model, to its settings; when the reply ended -->
          <div class="flex items-start gap-2">
            <button
              type="button"
              class="group flex min-w-0 flex-col items-start rounded-control px-1 py-0.5 text-left transition-colors hover:bg-surface-2"
              :title="t('settings.modelSettings')"
              @click="openModel"
            >
              <span class="flex max-w-full items-center gap-1 text-meta font-semibold text-ink">
                <span class="truncate">{{ model.name }}</span>
                <span v-if="sum.models.length > 1" class="flex-none text-ink-3"
                  >+{{ sum.models.length - 1 }}</span
                >
                <Icon
                  name="chevron"
                  :size="12"
                  class="flex-none text-ink-3 opacity-0 transition-opacity group-hover:opacity-100"
                />
              </span>
              <span class="max-w-full truncate font-mono text-micro text-ink-3">{{ model.ref }}</span>
            </button>
            <span class="ml-auto flex-none pt-0.5 text-micro tabular-nums text-ink-3">{{ whenText }}</span>
          </div>

          <div class="mt-2.5 grid grid-cols-4 gap-1.5">
            <div v-for="f in tiles" :key="f.label" class="min-w-0 rounded-control bg-surface-2 px-2.5 py-2">
              <div class="truncate text-micro text-ink-3">{{ f.label }}</div>
              <div class="mt-0.5 flex items-baseline gap-0.5 whitespace-nowrap">
                <span class="text-message leading-none font-black tabular-nums text-ink">{{ f.value }}</span>
                <span v-if="f.unit" class="text-micro font-semibold text-ink-3">{{ f.unit }}</span>
              </div>
            </div>
          </div>

          <TimeSplit :summary="sum" class="mt-3 px-1" />

          <CallBars :calls="reply.calls" class="mt-3 px-1">
            <template #idle>
              <div class="text-meta text-ink">
                <span class="font-semibold">{{ t('usage.input') }}</span>
                {{ ' ' }}<span class="tabular-nums">{{ sum.input.toLocaleString() }}</span>
                <span class="text-ink-3">
                  · {{ t('usage.context') }} {{ fmtTokens(sum.lastInput)
                  }}<template v-if="model.context"> / {{ fmtTokens(model.context) }}</template>
                </span>
              </div>
              <TokenChips
                :cached="sum.cached"
                :written="sum.written"
                :uncached="Math.max(0, sum.input - sum.cached - sum.written)"
                :output="sum.output"
              />
            </template>
          </CallBars>

          <div v-if="troubles.length || !view.sub" class="mt-1 flex items-center gap-2 px-1">
            <span v-if="troubles.length" class="text-micro text-warn">{{ troubles.join(' · ') }}</span>
            <button
              v-if="!view.sub"
              type="button"
              class="-mr-1 ml-auto inline-flex items-center gap-0.5 rounded-control px-1.5 py-0.5 text-micro font-semibold text-ink-2 transition-colors hover:bg-surface-2 hover:text-ink"
              @click="onClick"
            >
              {{ t('usage.open') }}
              <Icon name="chevron" :size="12" />
            </button>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
