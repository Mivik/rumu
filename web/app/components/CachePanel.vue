<script setup lang="ts">
// Cache hit-rate meters: one thin bar per assistant turn on the active path,
// plus a session-total headline. Single hue on a neutral track; every bar is
// directly labeled (text tokens), so color is never the sole carrier.
// The bars grow in and the headline counts up when the drawer opens.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../i18n'
import { cacheTotal, cacheTurns } from '../lib/store'

const totalPct = computed(() =>
  cacheTotal.value.prompt > 0 ? Math.round((cacheTotal.value.cached * 100) / cacheTotal.value.prompt) : 0,
)

/** The headline as shown: eases towards totalPct (instantly under reduced motion). */
const shownPct = ref(0)
let frame = 0

function countTo(target: number): void {
  cancelAnimationFrame(frame)
  const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches
  const from = shownPct.value
  if (reduce || from === target) {
    shownPct.value = target
    return
  }
  const t0 = performance.now()
  const step = (now: number): void => {
    const k = Math.min(1, (now - t0) / 700)
    shownPct.value = Math.round(from + (target - from) * (1 - (1 - k) ** 3))
    if (k < 1) frame = requestAnimationFrame(step)
  }
  frame = requestAnimationFrame(step)
}

watch(totalPct, countTo, { immediate: true })
onBeforeUnmount(() => cancelAnimationFrame(frame))

const totalSub = computed(() => {
  const c = cacheTotal.value
  return t('panels.cache.sub', {
    prompt: c.prompt,
    cached: c.cached,
    creation: c.creation,
    completion: c.completion,
    turns: c.turns,
  })
})

function pct(t: { prompt: number; cached: number }): number {
  return t.prompt > 0 ? Math.min(100, Math.round((t.cached * 100) / t.prompt)) : 0
}

function width(t: { prompt: number; cached: number }): string {
  return `${pct(t)}%`
}

const turns = computed(() => [...cacheTurns.value].reverse())
</script>

<template>
  <div>
    <div class="rounded-card bg-surface px-3 py-3 shadow-soft">
      <div class="font-display text-display font-semibold tabular-nums text-ink">{{ shownPct }}%</div>
      <div class="text-micro text-ink-3">{{ t('panels.cache.hitRate') }}</div>
      <div class="mt-0.5 font-mono text-micro text-ink-3">{{ totalSub }}</div>
    </div>
    <div v-if="turns.length" class="section-title mt-4 mb-1.5">{{ t('panels.cache.perTurn') }}</div>
    <div class="flex flex-col gap-2.5 px-3">
      <div v-for="(turn, i) in turns" :key="turn.nodeId" class="flex flex-col gap-1">
        <div class="flex justify-between text-micro tabular-nums text-ink-2">
          <span>{{ t('panels.cache.turn', { n: turn.index }) }}</span>
          <span
            >{{ pct(turn) }}% · {{ turn.cached }}/{{ turn.prompt }}
            <span v-if="turn.creation > 0" class="text-ink-3">(+{{ turn.creation }}w)</span></span
          >
        </div>
        <div
          class="h-1.5 overflow-hidden rounded-full bg-line"
          role="meter"
          :aria-valuenow="pct(turn)"
          aria-valuemin="0"
          aria-valuemax="100"
          :aria-label="t('panels.cache.meterLabel', { n: turn.index, pct: pct(turn) })"
        >
          <div class="bar-grow h-full rounded-full bg-brand" :style="{ width: width(turn), '--i': i }" />
        </div>
      </div>
    </div>
  </div>
</template>
