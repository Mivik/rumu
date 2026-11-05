<script setup lang="ts">
// Cache hit-rate meters: one thin bar per assistant turn on the active path,
// plus a session-total headline. Single hue on a neutral track; every bar is
// directly labeled (text tokens), so color is never the sole carrier.
// The bars grow in the first time a turn is seen (v-play-once); reopening the
// panel leaves them at rest.
import { computed } from 'vue'
import { t } from '../i18n'
import { cacheTotal, cacheTurns } from '../lib/store'
import { vPlayOnce } from '../lib/motion'

const totalPct = computed(() =>
  cacheTotal.value.prompt > 0 ? Math.round((cacheTotal.value.cached * 100) / cacheTotal.value.prompt) : 0,
)

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
      <div class="text-display font-bold tabular-nums text-ink">{{ totalPct }}%</div>
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
          <div
            v-play-once="{ key: `cache:${turn.nodeId}`, cls: 'bar-grow' }"
            class="h-full rounded-full bg-brand"
            :style="{ width: width(turn), '--i': i }"
          />
        </div>
      </div>
    </div>
  </div>
</template>
