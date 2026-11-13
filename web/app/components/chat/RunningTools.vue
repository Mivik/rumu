<script setup lang="ts">
// The calls of the live turn still waiting on their result: the last step
// of the run they belong to (the frontier), one compact row per call with
// its elapsed time and its live output tail (`tool.progress`). A call that
// has said nothing for STALL_MS turns amber and says how long it has been
// silent — the "is it stuck?" signal. It turns into a tool row of the run as
// soon as its tool_result node lands.
import { computed, onBeforeUnmount, ref } from 'vue'
import { t } from '../../i18n'
import type { ActivityRunning } from '../../lib/store'
import { state, useBranchView } from '../../lib/store'
import { argsOf, displayCall } from '../../lib/toolrows'
import { argSummary, fmtThought } from '../../lib/util'
import { toolIcon } from '../../lib/toolIcon'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ step: ActivityRunning }>()

const STALL_MS = 60_000

const view = useBranchView()
const now = ref(Date.now())
const timer = setInterval(() => (now.value = Date.now()), 1000)
onBeforeUnmount(() => clearInterval(timer))

const rows = computed(() => {
  const branch = view.branch.value
  return props.step.calls.map(({ call: raw, assistant }) => {
    const call = displayCall(raw)
    const args = argsOf(call)
    const progress = branch ? state.toolProgress[`${branch}:${call.call_id}`] : undefined
    // the call starts when the step that made it lands
    const start = Math.min(assistant.at, now.value)
    const quietFor = now.value - Math.max(start, progress?.at ?? 0)
    return {
      call,
      description: typeof args?.description === 'string' ? args.description.trim() : '',
      target: argSummary(call),
      progress: progress?.text ?? '',
      elapsed: fmtThought(now.value - start),
      stalled: quietFor >= STALL_MS ? fmtThought(quietFor) : null,
    }
  })
})

/** The last few lines, for the row's live tail. */
function tail(text: string): string {
  const lines = text.split('\n').filter((l) => l.trim())
  return lines.slice(-6).join('\n')
}
</script>

<template>
  <div class="flex flex-col gap-1">
    <div
      v-for="r in rows"
      :key="r.call.call_id"
      class="flex min-w-0 flex-col rounded-control border border-line-soft bg-surface-2/40 px-2.5 py-1"
    >
      <div class="flex min-w-0 items-center gap-2">
        <span class="dot-live size-1.5 flex-none rounded-full" :class="r.stalled ? 'bg-warn' : 'bg-brand'" />
        <Icon :name="toolIcon(r.call.name)" :size="12" class="flex-none text-tool" />
        <span class="max-w-[40%] flex-none truncate font-mono text-meta font-medium text-tool">{{
          r.call.name
        }}</span>
        <span v-if="r.description" class="flex-none truncate text-meta text-ink-2">{{ r.description }}</span>
        <span
          v-if="r.target"
          class="min-w-0 truncate font-mono text-meta"
          :class="r.description ? 'text-ink-3' : 'text-ink-2'"
          >{{ r.target }}</span
        >
        <span
          class="ml-auto flex-none text-micro tabular-nums"
          :class="r.stalled ? 'text-warn' : 'text-ink-3'"
          :title="t('common.running')"
        >
          {{ r.elapsed
          }}<template v-if="r.stalled"> · {{ t('tools.running.silent', { duration: r.stalled }) }}</template>
        </span>
      </div>
      <pre v-if="r.progress" class="code mt-1 max-h-40 overflow-auto">{{ tail(r.progress) }}</pre>
    </div>
  </div>
</template>
