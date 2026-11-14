<script setup lang="ts">
// One column per model call, in order. Above the axis its input, stacked by
// how it met the cache (read, written, neither) on one scale, so a growing
// context climbs and a cache miss stands out dark; below, its output. A
// warn dot over a column marks a call that ran without the reasoning before
// it. Calls of different replies sit a little apart. Columns keep a minimum
// width; past what fits, the chart scrolls sideways. A few calls sit centred
// (auto margins on the ends, which fall to zero once the row overflows).
//
// Hovering a column shows that call in the line under the chart; otherwise
// the line shows the `idle` slot. The line keeps three rows' height, so
// hovering never moves the layout.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { callRate } from '../../lib/store'
import type { CallStat } from '../../lib/store'
import { fmtDuration } from '../../lib/util'
import TokenChips from './TokenChips.vue'

const props = withDefaults(defineProps<{ calls: CallStat[]; size?: 'sm' | 'lg'; replyLabels?: boolean }>(), {
  size: 'sm',
  replyLabels: false,
})

const UP = computed(() => (props.size === 'lg' ? 104 : 52))
const DOWN = computed(() => (props.size === 'lg' ? 40 : 22))

const maxIn = computed(() => Math.max(1, ...props.calls.map((c) => c.input)))
const maxOut = computed(() => Math.max(1, ...props.calls.map((c) => c.output)))

/** Thinner gaps as the columns crowd; past their minimum width they scroll. */
const gap = computed(() => (props.calls.length > 60 ? 1 : 2))

const cols = computed(() =>
  props.calls.map((c, i) => {
    const k = UP.value / maxIn.value
    return {
      c,
      cached: c.cached * k,
      written: c.written * k,
      uncached: Math.max(0, c.input - c.cached - c.written) * k,
      // Under the axis's `pt-px`.
      out: c.output > 0 ? Math.max(1, ((DOWN.value - 1) * c.output) / maxOut.value) : 0,
      top: UP.value - c.input * k,
      apart: i > 0 && props.calls[i - 1]!.reply !== c.reply,
    }
  }),
)

const hovered = ref<number | null>(null)
const call = computed(() => (hovered.value === null ? null : (props.calls[hovered.value] ?? null)))

/** The call's position within its reply, 1-based. */
const callNo = computed(() => {
  const c = call.value
  if (!c) return 0
  return props.calls.filter((x, i) => x.reply === c.reply && i <= hovered.value!).length
})

const timing = computed(() => {
  const c = call.value
  if (!c) return ''
  const parts: string[] = []
  if (c.durMs !== null) parts.push(fmtDuration(c.durMs))
  if (c.ttftMs !== null) parts.push(`${t('usage.ttft')} ${fmtDuration(c.ttftMs)}`)
  if (c.reasonMs) parts.push(t('usage.thought', { ms: fmtDuration(c.reasonMs) }))
  const r = callRate(c)
  if (r !== null) parts.push(`${Math.round(r)} ${t('usage.unitRate')}`)
  return parts.join(' · ')
})
</script>

<template>
  <div>
    <div class="flex items-stretch overflow-x-auto" :style="{ gap: `${gap}px` }" @mouseleave="hovered = null">
      <div
        v-for="(col, i) in cols"
        :key="col.c.node.id"
        class="group relative flex min-w-1 flex-1 flex-col rounded-xs transition-colors first:ml-auto last:mr-auto hover:bg-surface-2"
        :class="[size === 'lg' ? 'max-w-10' : 'max-w-6', { 'ml-1.5': col.apart }]"
        @mouseenter="hovered = i"
      >
        <div class="relative flex flex-col justify-end" :style="{ height: `${UP}px` }">
          <span
            v-if="col.c.reset"
            class="absolute left-1/2 size-1.5 -translate-x-1/2 rounded-full bg-warn"
            :style="{ top: `${Math.max(0, col.top - 8)}px` }"
          />
          <div class="rounded-t-[2px] bg-ink-2" :style="{ height: `${col.uncached}px` }" />
          <div class="bg-warn/55" :style="{ height: `${col.written}px` }" />
          <div class="bg-pine/35 group-hover:bg-pine/55" :style="{ height: `${col.cached}px` }" />
        </div>
        <div class="h-px bg-line" />
        <div :style="{ height: `${DOWN}px` }" class="pt-px">
          <div class="rounded-b-[2px] bg-brand/75 group-hover:bg-brand" :style="{ height: `${col.out}px` }" />
        </div>
      </div>
    </div>

    <div class="mt-2 min-h-[3.9rem] border-t border-line-soft pt-2">
      <template v-if="call">
        <div class="flex flex-wrap items-baseline gap-x-1.5 text-meta">
          <span class="font-semibold text-ink">
            <template v-if="replyLabels">{{ t('usage.reply', { n: call.reply }) }} · </template
            >{{ t('usage.call', { n: callNo }) }}
          </span>
          <span class="min-w-0 truncate text-ink-3"
            >· {{ call.tools.join(', ') || t('usage.answered') }}</span
          >
          <span v-if="call.retries" class="text-warn">· {{ t('usage.retried', { n: call.retries }) }}</span>
          <span v-if="call.reset" class="text-warn">· {{ t('usage.reset') }}</span>
        </div>
        <div class="text-micro tabular-nums text-ink-2">
          {{ timing }} · {{ t('usage.input') }} {{ call.input.toLocaleString() }}
        </div>
        <TokenChips
          :cached="call.cached"
          :written="call.written"
          :uncached="Math.max(0, call.input - call.cached - call.written)"
          :output="call.output"
        />
      </template>
      <slot v-else name="idle" />
    </div>
  </div>
</template>
