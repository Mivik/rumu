<script setup lang="ts">
// A token breakdown as a row of swatches: what of the input came from the
// cache, was written to it or was neither, and the output. The swatches are
// the CallBars colors, so the row doubles as the chart's legend.
import { t } from '../../i18n'
import { fmtTokens } from '../../lib/util'

defineProps<{ cached: number; written: number; uncached: number; output: number }>()
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-micro tabular-nums text-ink-2">
    <span class="inline-flex items-center gap-1.5"
      ><i class="size-2 rounded-xs bg-pine/35" />{{ t('usage.cached') }} {{ fmtTokens(cached) }}</span
    >
    <span v-if="written" class="inline-flex items-center gap-1.5"
      ><i class="size-2 rounded-xs bg-warn/55" />{{ t('usage.written') }} {{ fmtTokens(written) }}</span
    >
    <span class="inline-flex items-center gap-1.5"
      ><i class="size-2 rounded-xs bg-ink-2" />{{ t('usage.uncached') }} {{ fmtTokens(uncached) }}</span
    >
    <span class="inline-flex items-center gap-1.5"
      ><i class="size-2 rounded-xs bg-brand/75" />{{ t('usage.output') }} {{ fmtTokens(output) }}</span
    >
  </div>
</template>
