<script setup lang="ts">
// Where the time of a reply (or a range of them) went: one bar split into the
// model's wait for its first token, its generation, the tools and everything
// between, labeled below, with the generation speed at the end of the legend.
// The two model parts share the coral, the wait as its tint.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Summary } from '../../lib/store'
import { fmtDuration } from '../../lib/util'

const props = defineProps<{ summary: Summary }>()

const parts = computed(() =>
  [
    { key: 'wait', label: t('usage.wait'), ms: props.summary.waitMs, cls: 'bg-brand/40' },
    { key: 'gen', label: t('usage.gen'), ms: props.summary.genMs, cls: 'bg-brand' },
    { key: 'tool', label: t('usage.tool'), ms: props.summary.toolMs, cls: 'bg-tool' },
    { key: 'other', label: t('usage.other'), ms: props.summary.otherMs, cls: 'bg-line' },
  ].filter((p) => p.ms > 0),
)

const rate = computed(() => {
  const r = props.summary.rate
  if (r === null) return null
  return `${r >= 10 ? Math.round(r) : r.toFixed(1)} ${t('usage.unitRate')}`
})
</script>

<template>
  <div v-if="parts.length">
    <div class="flex h-2 gap-0.5 overflow-hidden rounded-full">
      <div v-for="p in parts" :key="p.key" :class="p.cls" :style="{ flex: `${p.ms} 1 0` }" />
    </div>
    <div class="mt-1.5 flex flex-wrap items-center gap-x-3.5 gap-y-1 text-micro text-ink-2">
      <span v-for="p in parts" :key="p.key" class="inline-flex items-center gap-1.5">
        <i class="size-2 rounded-xs" :class="p.cls" />
        {{ p.label }}
        <b class="font-semibold tabular-nums text-ink">{{ fmtDuration(p.ms) }}</b>
      </span>
      <span v-if="rate" class="ml-auto tabular-nums text-ink-3" :title="t('usage.rate')">{{ rate }}</span>
    </div>
  </div>
</template>
