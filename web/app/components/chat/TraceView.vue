<script setup lang="ts">
// The trace view: the open session's raw trajectory. Every node on the
// active path in order, the hook firings seen since this page loaded between
// them, and each sub-agent's own rows folded under the call that started it.
// A row opens into its full JSON. Like the conversation, it opens at the end
// and follows new rows while you are there.
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { t } from '../../i18n'
import { pathNodes, state, trace } from '../../lib/store'
import type { TraceKind } from '../../lib/store'
import TraceRows from './TraceRows.vue'

const KINDS: TraceKind[] = ['user', 'assistant', 'tool', 'note', 'marker', 'hook']

const open = reactive(new Set<string>())
const hidden = reactive(new Set<TraceKind>())

function toggleKind(k: TraceKind): void {
  if (hidden.has(k)) hidden.delete(k)
  else hidden.add(k)
}

/** call id → tool name across the whole tree (sub-agents' calls included). */
const calls = computed(() => {
  const map = new Map<string, string>()
  for (const n of Object.values(state.nodes)) {
    if (n.body.type === 'assistant') for (const c of n.body.tool_calls) map.set(c.call_id, c.name)
  }
  return map
})

const hookCount = computed(() => trace.value.filter((r) => r.kind === 'hook').length)

const scrollEl = ref<HTMLElement | null>(null)
const nearBottom = ref(true)

function onScroll(): void {
  const el = scrollEl.value
  if (el) nearBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 90
}

function toEnd(): void {
  void nextTick(() => {
    const el = scrollEl.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

// Another session starts closed, at its end.
watch(
  () => state.sessionId,
  () => {
    open.clear()
    nearBottom.value = true
    toEnd()
  },
  { immediate: true },
)

// New rows follow the end only while you are there; opening a row never
// scrolls.
watch(trace, () => {
  if (nearBottom.value) toEnd()
})
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div ref="scrollEl" class="scroll-fade flex-1 overflow-y-auto pt-3 pb-2" @scroll="onScroll">
      <div
        :key="state.sessionId ?? ''"
        class="view-in mx-auto flex max-w-[var(--col-max)] flex-col px-2 sm:px-4"
      >
        <div class="mb-2 flex flex-wrap items-center gap-x-3 gap-y-1.5 px-2">
          <span class="text-micro text-ink-3 tabular-nums">
            {{ t('chat.trace.nodes', { count: pathNodes.length }) }}
            <template v-if="hookCount"> · {{ t('chat.trace.hooks', { count: hookCount }) }}</template>
          </span>
          <span
            class="ml-auto flex flex-wrap items-center gap-1"
            role="group"
            :aria-label="t('chat.trace.show')"
          >
            <button
              v-for="k in KINDS"
              :key="k"
              type="button"
              class="rounded-full px-2 py-0.5 font-mono text-micro transition-colors"
              :class="
                hidden.has(k)
                  ? 'text-ink-3 line-through hover:text-ink-2'
                  : 'bg-surface-2 text-ink-2 hover:text-ink'
              "
              :aria-pressed="!hidden.has(k)"
              @click="toggleKind(k)"
            >
              {{ k }}
            </button>
          </span>
        </div>
        <TraceRows v-if="trace.length" :rows="trace" :open="open" :hidden="hidden" :calls="calls" />
        <div v-else class="px-2 py-10 text-center text-meta text-ink-3">{{ t('chat.trace.empty') }}</div>
      </div>
    </div>
  </div>
</template>
