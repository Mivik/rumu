<script setup lang="ts">
// Per-reply usage line: a quiet "N tok · X tok/s" under an assistant turn.
// Hover, focus or tap opens a record card for the latest request (model, time,
// tokens, cache, latency, speed, and the turn's totals).
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

let closeTimer: ReturnType<typeof setTimeout> | null = null
function enter(): void {
  if (closeTimer !== null) {
    clearTimeout(closeTimer)
    closeTimer = null
  }
  hover.value = true
}
function leave(): void {
  if (closeTimer !== null) clearTimeout(closeTimer)
  closeTimer = setTimeout(() => {
    closeTimer = null
    hover.value = false
  }, 140)
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
    steps: assistants.length,
    turn: { prompt, completion },
  }
})

/** No usage on the path (a fresh branch): drop any open/pinned state. */
watch(stats, (s) => {
  if (!s) {
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

const requestFields = computed(() => {
  const s = stats.value
  if (!s) return []
  return [
    { label: t('usage.input'), value: t('usage.tokens', { n: num(s.usage.prompt_tokens) }) },
    { label: t('usage.output'), value: t('usage.tokens', { n: num(s.usage.completion_tokens) }) },
    {
      label: t('usage.cacheRead'),
      value: s.cachePct === null ? '—' : `${num(s.usage.cached_tokens)} (${s.cachePct}%)`,
    },
    {
      label: t('usage.cacheWrite'),
      value: t('usage.tokens', { n: num(s.usage.cache_creation_tokens) }),
    },
    { label: t('usage.latency'), value: s.dur === null ? '—' : fmtDuration(s.dur) },
    {
      label: t('usage.speed'),
      value: s.rate === null ? '—' : t('usage.tokPerSecond', { rate: fmtRate(s.rate) }),
    },
  ]
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
    @focusin="enter"
    @focusout="leave"
    @keydown.escape="pinned = false"
  >
    <button
      type="button"
      class="-mx-1 inline-flex h-5 items-center gap-1.5 rounded-control px-1 font-mono text-micro tabular-nums text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink-2"
      :aria-label="t('usage.aria')"
      :aria-expanded="open"
      :aria-describedby="open ? TIP_ID : undefined"
      @click="pinned = !pinned"
    >
      <span class="text-ink-2">{{ t('usage.tokens', { n: fmtTokens(stats.usage.completion_tokens) }) }}</span>
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
          class="z-30 w-80 max-w-[calc(100vw-2rem)] overflow-hidden rounded-card border border-line-soft bg-surface shadow-pop"
          @mouseenter="enter"
          @mouseleave="leave"
        >
          <!-- record header: the request's model and timestamp -->
          <div class="border-b border-line-soft bg-surface-2/60 px-3.5 py-2.5">
            <div
              class="truncate font-display text-secondary font-semibold text-ink"
              :title="stats.body.model"
            >
              {{ stats.body.model }}
            </div>
            <div class="mt-0.5 font-mono text-micro text-ink-3">{{ whenText }}</div>
          </div>

          <div class="px-3.5 py-3">
            <div class="flex items-center gap-2">
              <span class="text-micro font-semibold text-ink-2">{{ t('usage.request') }}</span>
              <span class="h-px flex-1 bg-line-soft" />
            </div>
            <div class="mt-2 grid grid-cols-3 gap-x-3 gap-y-2.5">
              <div v-for="f in requestFields" :key="f.label">
                <div class="text-micro text-ink-3">{{ f.label }}</div>
                <div class="mt-0.5 font-mono text-meta tabular-nums text-ink">{{ f.value }}</div>
              </div>
            </div>
          </div>

          <div class="border-t border-line-soft px-3.5 py-3">
            <div class="flex items-center gap-2">
              <span class="text-micro font-semibold text-ink-2">{{ t('usage.turn') }}</span>
              <span class="h-px flex-1 bg-line-soft" />
            </div>
            <div class="mt-2 grid grid-cols-2 gap-x-3 gap-y-2.5">
              <div>
                <div class="text-micro text-ink-3">{{ t('usage.endToEnd') }}</div>
                <div class="mt-0.5 font-mono text-meta tabular-nums text-ink">
                  {{ stats.endToEnd === null ? '—' : fmtDuration(stats.endToEnd) }}
                </div>
              </div>
              <div>
                <div class="text-micro text-ink-3">{{ t('usage.generation') }}</div>
                <div class="mt-0.5 font-mono text-meta tabular-nums text-ink">
                  {{ stats.generation === null ? '—' : fmtDuration(stats.generation) }}
                </div>
              </div>
              <div>
                <div class="text-micro text-ink-3">{{ t('usage.steps') }}</div>
                <div class="mt-0.5 font-mono text-meta tabular-nums text-ink">
                  {{ stats.steps }}
                </div>
              </div>
              <div>
                <div class="text-micro text-ink-3">{{ t('usage.tokensLabel') }}</div>
                <div class="mt-0.5 font-mono text-meta tabular-nums text-ink">
                  {{
                    t('usage.inOut', {
                      input: num(stats.turn.prompt),
                      output: num(stats.turn.completion),
                    })
                  }}
                </div>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
