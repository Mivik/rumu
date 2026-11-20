<script lang="ts">
import { reactive } from 'vue'

/** Running rows opened, by call id — shared by the run's folded and opened
 * copies of the list. */
const opened = reactive(new Set<string>())
</script>

<script setup lang="ts">
// The calls of the live turn still waiting on their result: the last step
// of the run they belong to (the frontier), one compact row per call with
// its elapsed time and its live output tail (`tool.progress`). A call that
// has said nothing for STALL_MS turns amber and says how long it has been
// silent — the "is it stuck?" signal. It turns into a tool row of the run as
// soon as its tool_result node lands.
// A row opens like a finished one: a bash its whole command and output, any
// other call its args and output; a sub-agent's chip opens its transcript
// beside the chat.
import { computed, onBeforeUnmount, ref } from 'vue'
import { t } from '../../i18n'
import type { ActivityRunning } from '../../lib/store'
import { findAgentChild, openAgentView, state, useBranchView } from '../../lib/store'
import { argsOf, bashExtraArgs, bashTimeout, displayCall } from '../../lib/toolrows'
import { vFollow } from '../../lib/motion'
import { argSummary, fmtThought, shortId } from '../../lib/util'
import { toolIcon } from '../../lib/toolIcon'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import BashView from './BashView.vue'
import JsonTree from './JsonTree.vue'

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
    const child = call.name === 'agent' ? findAgentChild(call.call_id) : null
    const timeout = call.name === 'bash' ? bashTimeout(args) : null
    return {
      call,
      args,
      child,
      open: opened.has(call.call_id),
      description: typeof args?.description === 'string' ? args.description.trim() : '',
      target: argSummary(call),
      progress: progress?.text ?? '',
      elapsed: fmtThought(now.value - start),
      extra: bashExtraArgs(args),
      timeout: timeout === null ? null : fmtThought(timeout * 1000),
      stalled: quietFor >= STALL_MS ? fmtThought(quietFor) : null,
    }
  })
})

type Row = (typeof rows.value)[number]

function toggle(r: Row): void {
  if (r.open) opened.delete(r.call.call_id)
  else opened.add(r.call.call_id)
}

/** A string arg of the call: a `code` script, a `bash` command. */
function strArg(r: Row, key: string): string {
  const raw = r.args?.[key]
  return typeof raw === 'string' ? raw.trim() : ''
}

/** An open bash row: the command is in its body, like a finished one. */
function isShellOpen(r: Row): boolean {
  return r.call.name === 'bash' && r.open
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
      class="flex min-w-0 flex-col overflow-hidden rounded-control border border-line-soft bg-surface-2/40"
    >
      <button
        type="button"
        class="flex w-full min-w-0 items-center gap-2 px-2.5 py-1 text-left transition-colors"
        :class="r.open ? 'bg-surface-2' : 'hover:bg-surface-2/70'"
        :aria-expanded="r.open"
        @click="toggle(r)"
      >
        <span class="flex h-4 flex-none items-center">
          <span class="dot-live size-1.5 rounded-full" :class="r.stalled ? 'bg-warn' : 'bg-brand'" />
        </span>
        <span class="flex h-4 flex-none items-center">
          <Icon :name="toolIcon(r.call.name)" :size="12" class="text-tool" />
        </span>
        <span class="max-w-[40%] flex-none truncate font-mono text-meta font-medium text-tool">{{
          r.call.name === 'bash' && !r.open && !r.description ? '$' : r.call.name
        }}</span>
        <span v-if="r.description" class="flex-none truncate text-meta text-ink-2">{{ r.description }}</span>
        <span
          v-if="r.target && !isShellOpen(r)"
          class="min-w-0 truncate font-mono text-meta"
          :class="r.description ? 'text-ink-3' : 'text-ink-2'"
          >{{ r.target }}</span
        >
        <span class="ml-auto flex flex-none items-center gap-2">
          <Chip
            v-if="r.child"
            icon="agent"
            tone="agent"
            clickable
            :title="t('tools.row.subAgentTitle', { id: shortId(r.child.id) })"
            @click.stop="openAgentView(r.child.id)"
          >
            {{ t('tools.row.openSubAgent') }}
          </Chip>
          <span
            class="text-micro tabular-nums"
            :class="r.stalled ? 'text-warn' : 'text-ink-3'"
            :title="t('common.running')"
          >
            {{ r.elapsed }}<template v-if="r.timeout"> / {{ r.timeout }}</template
            ><template v-if="r.stalled"> · {{ t('tools.running.silent', { duration: r.stalled }) }}</template>
          </span>
          <Icon
            name="chevron"
            :size="12"
            class="transition-transform duration-100"
            :class="r.open ? 'rotate-90 text-ink-2' : 'text-ink-3/70'"
          />
        </span>
      </button>

      <template v-if="r.open">
        <BashView
          v-if="r.call.name === 'bash'"
          class="reveal"
          :output="r.progress"
          :failed="false"
          running
          :command="strArg(r, 'command')"
          :args="r.extra"
        />
        <div v-else class="reveal flex flex-col gap-2 pt-1 pr-2.5 pb-2.5 pl-[30px]">
          <div v-if="r.call.name === 'code'">
            <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.script') }}</div>
            <pre class="code">{{ strArg(r, 'code') }}</pre>
          </div>
          <div v-else>
            <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.args') }}</div>
            <div class="code whitespace-normal"><JsonTree :value="r.call.args" bare /></div>
          </div>
          <div v-if="r.progress">
            <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.output') }}</div>
            <pre v-follow="true" class="code max-h-80 overflow-auto">{{ r.progress }}</pre>
          </div>
        </div>
      </template>
      <pre v-else-if="r.progress" class="code mx-2.5 mb-1.5 max-h-40 overflow-auto">{{
        tail(r.progress)
      }}</pre>
    </div>
  </div>
</template>
