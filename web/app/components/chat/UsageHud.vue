<script setup lang="ts">
// Per-reply usage line: a quiet "N tok · X tok/s" under an assistant turn.
// Hover, focus or tap opens a record card for the latest request (model, time,
// tokens, cache, latency, speed, and the turn's totals).
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
import { pathNodes } from '../../lib/store'
import type { TimelineGroup } from '../../lib/store'
import type { Node as RumuNode } from '../../lib/types'
import { fmtDuration, fmtTokens } from '../../lib/util'

const props = defineProps<{ group: TimelineGroup }>()

type AsstBody = Extract<RumuNode['body'], { type: 'assistant' }>

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

/** Focus and tap: no dwell, since there is no accidental sweep to guard against. */
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

/** Tap toggles the pinned card; drop any pending dwell so it cannot reopen later. */
function togglePin(): void {
  cancelOpen()
  pinned.value = !pinned.value
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

// ---- the latest request of this reply ----

const stats = computed(() => {
  const assistants = props.group.assistants
  const node = assistants[assistants.length - 1]
  if (!node) return null
  const body = node.body as AsstBody
  const usage = body.usage

  // The prompt that opened this turn: end-to-end runs prompt → this reply.
  // A branch without a prompt of its own (a sub-agent child) has no turn start.
  const nodes = pathNodes.value
  let at = -1
  for (let i = nodes.length - 1; i >= 0; i--) {
    if (nodes[i].id === node.id) {
      at = i
      break
    }
  }
  let start = -1
  if (at >= 0) {
    for (let i = at; i >= 0; i--) {
      if (nodes[i].body.type === 'user') {
        start = i
        break
      }
    }
  }
  const endToEnd = start >= 0 ? Math.max(0, node.at - nodes[start].at) : null

  // Totals of every step in the turn.
  let prompt = 0
  let completion = 0
  let genMs = 0
  let haveGen = false
  for (const n of assistants) {
    const b = n.body as AsstBody
    prompt += b.usage.prompt_tokens
    completion += b.usage.completion_tokens
    if (n.dur_ms != null) {
      genMs += n.dur_ms
      haveGen = true
    }
  }

  const dur = node.dur_ms
  const rate =
    dur != null && dur > 0 && usage.completion_tokens > 0 ? (usage.completion_tokens * 1000) / dur : null
  const cachePct =
    usage.prompt_tokens > 0 ? Math.round((usage.cached_tokens * 100) / usage.prompt_tokens) : null

  return {
    node,
    body,
    usage,
    dur: dur ?? null,
    rate,
    cachePct,
    endToEnd,
    generation: haveGen ? genMs : null,
    calls: assistants.length,
    turn: { prompt, completion },
  }
})

/** No usage on the path (a fresh branch): drop any open/pinned state. */
watch(stats, (s) => {
  if (!s) {
    cancelOpen()
    hover.value = false
    pinned.value = false
  }
})

const whenText = computed(() => {
  const s = stats.value
  if (!s) return ''
  return new Date(s.node.at).toLocaleString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
})

function num(n: number): string {
  return n.toLocaleString()
}

function fmtRate(rate: number): string {
  return rate >= 10 ? String(Math.round(rate)) : rate.toFixed(1)
}

const rateText = computed(() => {
  const s = stats.value
  if (!s || s.rate === null) return '—'
  return t('usage.tokPerSecond', { rate: fmtRate(s.rate) })
})

const headline = computed(() => {
  const s = stats.value
  if (!s) return []
  return [
    { label: t('usage.input'), value: num(s.usage.prompt_tokens), unit: t('usage.unitTokens') },
    { label: t('usage.output'), value: num(s.usage.completion_tokens), unit: t('usage.unitTokens') },
    {
      label: t('usage.speed'),
      value: s.rate === null ? '—' : fmtRate(s.rate),
      unit: s.rate === null ? '' : t('usage.unitRate'),
    },
  ]
})

const details = computed(() => {
  const s = stats.value
  if (!s) return []
  return [
    {
      label: t('usage.cacheRead'),
      value: s.cachePct === null ? '—' : `${num(s.usage.cached_tokens)} (${s.cachePct}%)`,
    },
    {
      label: t('usage.cacheWrite'),
      value: `${num(s.usage.cache_creation_tokens)} ${t('usage.unitTokens')}`,
    },
    { label: t('usage.latency'), value: s.dur === null ? '—' : fmtDuration(s.dur) },
    { label: t('usage.endToEnd'), value: s.endToEnd === null ? '—' : fmtDuration(s.endToEnd) },
    { label: t('usage.generation'), value: s.generation === null ? '—' : fmtDuration(s.generation) },
    { label: t('usage.modelCalls'), value: String(s.calls) },
  ]
})

const turnTokens = computed(() => {
  const s = stats.value
  if (!s) return ''
  return t('usage.inOut', { input: num(s.turn.prompt), output: num(s.turn.completion) })
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
    v-if="stats"
    ref="pill"
    class="relative inline-flex"
    @mouseenter="enter"
    @mouseleave="leave"
    @focusin="enterNow"
    @focusout="leave"
    @keydown.escape="pinned = false"
  >
    <button
      type="button"
      class="-mx-1 inline-flex h-5 items-center gap-1.5 rounded-control px-1 text-micro tabular-nums text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink-2"
      :aria-label="t('usage.aria')"
      :aria-expanded="open"
      :aria-describedby="open ? TIP_ID : undefined"
      @click="togglePin"
    >
      <span class="font-bold text-ink-2">{{
        t('usage.tokens', { n: fmtTokens(stats.usage.completion_tokens) })
      }}</span>
      <span aria-hidden="true" class="h-3 w-px bg-line" />
      <span>{{ rateText }}</span>
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
          class="z-30 w-[22rem] max-w-[calc(100vw-2rem)] rounded-card border border-line-soft bg-surface p-3 shadow-pop"
          @mouseenter="enter"
          @mouseleave="leave"
        >
          <!-- record header: the request's model and timestamp -->
          <div class="flex items-baseline gap-3 px-1">
            <div
              class="min-w-0 flex-1 truncate font-mono text-meta font-semibold text-ink"
              :title="stats.body.model"
            >
              {{ stats.body.model }}
            </div>
            <div class="flex-none text-micro tabular-nums text-ink-3">{{ whenText }}</div>
          </div>

          <!-- the three numbers worth reading at a glance -->
          <div class="mt-2.5 grid grid-cols-3 gap-1.5">
            <div v-for="f in headline" :key="f.label" class="rounded-control bg-surface-2 px-2.5 py-2">
              <div class="text-micro text-ink-3">{{ f.label }}</div>
              <div class="mt-0.5 flex items-baseline gap-1">
                <span class="text-message leading-none font-black tabular-nums text-ink">{{ f.value }}</span>
                <span v-if="f.unit" class="text-micro font-semibold text-ink-3">{{ f.unit }}</span>
              </div>
            </div>
          </div>

          <dl class="mt-3 grid grid-cols-2 gap-x-3 gap-y-2.5 px-1">
            <div v-for="d in details" :key="d.label">
              <dt class="text-micro text-ink-3">{{ d.label }}</dt>
              <dd class="mt-0.5 text-meta font-semibold tabular-nums text-ink">{{ d.value }}</dd>
            </div>
            <!-- Single-step turns repeat the headline numbers; only sum when there is more. -->
            <div v-if="stats.calls > 1" class="col-span-2">
              <dt class="text-micro text-ink-3">{{ t('usage.turn') }} · {{ t('usage.tokensLabel') }}</dt>
              <dd class="mt-0.5 text-meta font-semibold tabular-nums text-ink">{{ turnTokens }}</dd>
            </div>
          </dl>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
