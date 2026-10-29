<script setup lang="ts">
// The live, still-running tool calls of the current turn: one compact row per
// pending call with its live output tail (`tool.progress`). A call moves into
// the folded ActivityRun as soon as its tool_result node lands.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { TimelineGroup } from '../../lib/store'
import type { ToolCall } from '../../lib/types'
import { pathNodes, state } from '../../lib/store'
import { argSummary } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ group: TimelineGroup }>()

interface RunningRow {
  call: ToolCall
  progress: string
}

const rows = computed<RunningRow[]>(() => {
  const answered = new Set<string>()
  for (const n of pathNodes.value) {
    if (n.body.type === 'tool_result') answered.add(n.body.call_id)
  }
  const branch = state.activeBranch
  if (!branch) return []
  const out: RunningRow[] = []
  for (const a of props.group.assistants) {
    if (a.body.type !== 'assistant') continue
    for (const call of a.body.tool_calls) {
      if (answered.has(call.call_id)) continue
      const progress = state.toolProgress[`${branch}:${call.call_id}`]?.text ?? ''
      out.push({ call, progress })
    }
  }
  return out
})

function target(call: ToolCall): string {
  return argSummary(call)
}

/** The last few lines, for the row's live tail. */
function tail(text: string): string {
  const lines = text.split('\n').filter((l) => l.trim())
  return lines.slice(-6).join('\n')
}
</script>

<template>
  <div v-if="rows.length" class="flex flex-col gap-1">
    <div
      v-for="r in rows"
      :key="r.call.call_id"
      class="flex min-w-0 flex-col rounded-control border border-line-soft bg-surface-2/40 px-2.5 py-1"
    >
      <div class="flex min-w-0 items-center gap-2">
        <span class="dot-live size-1.5 flex-none rounded-full bg-brand" />
        <Icon name="tool" :size="12" class="flex-none text-tool" />
        <span class="max-w-[40%] flex-none truncate font-mono text-meta font-medium text-tool">{{
          r.call.name
        }}</span>
        <span v-if="target(r.call)" class="min-w-0 truncate font-mono text-meta text-ink-2">{{
          target(r.call)
        }}</span>
        <span class="ml-auto flex-none text-micro text-ink-3">{{ t('common.running') }}</span>
      </div>
      <pre v-if="r.progress" class="code mt-1 max-h-40 overflow-auto">{{ tail(r.progress) }}</pre>
    </div>
  </div>
</template>
