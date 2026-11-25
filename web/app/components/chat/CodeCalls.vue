<script setup lang="ts">
// Internal tool calls made by one `code` script: a compact list under the
// script body. Rows expand to their args/output. While the script runs
// (`live`) the sub-agents it forked and is still waiting on follow as
// running rows, so the list keeps its shape when the script ends.
// Each invoke is an ordinary ToolResult node whose `invoke.parent` names the
// `code` call; they nest here instead of showing as chat rows. While the
// row is shut, the files the script changed stand after it as EditCards.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Branch, FilePatch, Tagged } from '../../lib/types'
import type { ToolRowInfo } from '../../lib/store'
import { findAgentChild, openAgentView, scriptAgents } from '../../lib/store'
import { argSummary, fmtThought, shortId } from '../../lib/util'
import { taskIdOf } from '../../lib/tasks'
import { bashExtraArgs, displayCall } from '../../lib/toolrows'
import { patchOfData } from '../../lib/diff'
import { toolIcon } from '../../lib/toolIcon'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import BashView from './BashView.vue'
import FileDiff from './FileDiff.vue'
import FileView from './FileView.vue'
import JsonTree from './JsonTree.vue'
import TaskChip from './TaskChip.vue'

const props = defineProps<{
  invokes: ToolRowInfo[]
  /** The script is still running: its branch, its `code` call, the clock. */
  live?: { branch: string; callId: string; now: number }
}>()

/** One invoke node under this `code` call, flattened for display. */
interface CodeCallRow {
  call_id: string
  name: string
  args: unknown
  blocked: string | null
  is_error: boolean
  output: string
  patch: Tagged | null
  dur_ms: number
  /** A `background: true` call's task. */
  task_id: string | null
  /** The sub-agent an `agent` call forked. */
  child: Branch | null
}

const calls = computed<CodeCallRow[]>(() => {
  const out: CodeCallRow[] = []
  for (const { result: n } of props.invokes) {
    const b = n.body
    if (b.type !== 'tool_result' || !b.invoke) continue
    out.push({
      call_id: b.call_id,
      ...displayCall({ name: b.invoke.name, args: b.invoke.args }),
      blocked: b.invoke.blocked ?? null,
      is_error: b.is_error,
      output: b.content.text,
      patch: b.content.data ?? null,
      dur_ms: n.dur_ms ?? 0,
      task_id: b.is_error ? null : taskIdOf(b.content.text),
      child: b.invoke.name === 'agent' ? findAgentChild(b.call_id) : null,
    })
  }
  return out
})

/** Sub-agents the running script forked whose call has not returned yet. */
const pending = computed(() => {
  const live = props.live
  if (!live) return []
  const done = new Set(calls.value.map((c) => c.call_id))
  return scriptAgents(live.branch, live.callId)
    .filter((b) => b.kind.type === 'agent' && !done.has(b.kind.call_id))
    .map((b) => ({
      branch: b,
      task: b.kind.type === 'agent' ? (b.kind.task.split('\n')[0]?.trim() ?? '') : '',
      elapsed: fmtThought(Math.max(0, live.now - b.created_at)),
    }))
})
const expanded = ref<Set<string>>(new Set())

type Status = 'ok' | 'error' | 'blocked'

function status(c: CodeCallRow): Status {
  return c.blocked ? 'blocked' : c.is_error ? 'error' : 'ok'
}
function statusLabel(c: CodeCallRow): string {
  return t(`tools.row.status.${status(c)}`)
}
function isOpen(id: string): boolean {
  return expanded.value.has(id)
}
function toggle(id: string): void {
  if (expanded.value.has(id)) expanded.value.delete(id)
  else expanded.value.add(id)
}
function summary(c: CodeCallRow): string {
  return argSummary({ name: c.name, args: c.args })
}
function output(c: CodeCallRow): string {
  return c.blocked ?? c.output
}

/** A successful write/edit expands to its diff, a read to its file and a
 * bash to its terminal, like a direct call's row. */
function patchOf(c: CodeCallRow): FilePatch | null {
  return status(c) === 'ok' ? patchOfData(c.patch) : null
}
function argsOf(c: CodeCallRow): Record<string, unknown> | null {
  const a = c.args
  return a && typeof a === 'object' && !Array.isArray(a) ? (a as Record<string, unknown>) : null
}
function pathOf(c: CodeCallRow): string {
  const p = argsOf(c)?.path
  return typeof p === 'string' ? p : ''
}
/** A bash call's command, whole in its opened body. */
function commandOf(c: CodeCallRow): string {
  const cmd = argsOf(c)?.command
  return typeof cmd === 'string' ? cmd.trim() : ''
}

const iconTone: Record<Status, string> = {
  ok: 'text-tool',
  error: 'text-danger',
  blocked: 'text-danger',
}
</script>

<template>
  <div v-if="calls.length || pending.length" class="flex flex-col gap-1.5" :class="{ 'mt-2': !live }">
    <div
      class="flex flex-col divide-y divide-line-soft overflow-hidden rounded-control border border-line-soft"
    >
      <div
        v-for="c in calls"
        :key="c.call_id"
        class="flex flex-col"
        :class="{ 'bg-danger-soft/30': status(c) !== 'ok' }"
      >
        <button
          class="flex w-full min-w-0 items-center gap-2 px-2 py-1 text-left transition-colors"
          :class="isOpen(c.call_id) ? 'bg-surface-2' : 'hover:bg-surface-2/70'"
          :title="`${c.name} ${summary(c)} → ${statusLabel(c)}`"
          @click="toggle(c.call_id)"
        >
          <Icon
            :name="status(c) === 'ok' ? toolIcon(c.name) : status(c) === 'blocked' ? 'blocked' : 'error'"
            :size="12"
            :class="iconTone[status(c)]"
          />
          <span
            class="font-mono text-micro font-medium"
            :class="status(c) === 'ok' ? 'text-tool' : 'text-danger'"
          >
            {{ c.name === 'bash' && !isOpen(c.call_id) ? '$' : c.name }}
          </span>
          <span
            v-if="summary(c) && !(c.name === 'bash' && isOpen(c.call_id))"
            class="min-w-0 truncate font-mono text-micro text-ink-2"
            >{{ summary(c) }}</span
          >
          <span class="ml-auto flex flex-none items-center gap-2">
            <TaskChip v-if="c.task_id" :id="c.task_id" />
            <span v-else class="text-micro text-ink-3">{{ c.dur_ms }} ms</span>
            <span
              v-if="c.blocked"
              class="rotate-[-4deg] rounded-xs border-[1.5px] border-danger/60 px-1.5 text-micro font-bold tracking-wide text-danger"
              >{{ t('tools.row.status.blocked') }}</span
            >
            <span v-else-if="c.is_error" class="text-micro font-semibold text-danger">{{
              t('tools.row.status.error')
            }}</span>
            <Icon
              name="chevron"
              :size="12"
              class="transition-transform duration-100"
              :class="isOpen(c.call_id) ? 'rotate-90 text-ink-2' : 'text-ink-3/70'"
            />
          </span>
        </button>
        <Chip
          v-if="c.child"
          class="mt-0.5 mb-1.5 ml-[28px] self-start"
          icon="agent"
          icon-after="external"
          tone="agent"
          clickable
          :title="t('tools.row.subAgentTitle', { id: shortId(c.child.id) })"
          @click="openAgentView(c.child.id)"
        >
          {{ t('tools.row.openSubAgent') }}
        </Chip>
        <FileDiff
          v-if="isOpen(c.call_id) && patchOf(c)"
          class="reveal"
          :patch="patchOf(c)"
          :path="pathOf(c)"
          :args="argsOf(c)"
        />
        <FileView
          v-else-if="isOpen(c.call_id) && c.name === 'read' && status(c) === 'ok'"
          class="reveal"
          :text="c.output"
          :path="pathOf(c)"
          :args="argsOf(c)"
        />
        <BashView
          v-else-if="isOpen(c.call_id) && c.name === 'bash'"
          class="reveal"
          :output="output(c)"
          :failed="status(c) !== 'ok'"
          :command="commandOf(c)"
          :args="bashExtraArgs(argsOf(c))"
        />
        <div v-else-if="isOpen(c.call_id)" class="reveal flex flex-col gap-1.5 pt-1 pr-2 pb-2 pl-[28px]">
          <div>
            <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.args') }}</div>
            <div class="code whitespace-normal"><JsonTree :value="c.args" bare /></div>
          </div>
          <div>
            <div class="mb-1 text-micro font-semibold text-ink-3">
              {{ c.is_error ? t('tools.detail.outputError') : t('tools.detail.output') }}
            </div>
            <pre class="code" :class="{ 'border-danger/40 text-danger': c.is_error }">{{
              output(c) || t('common.empty')
            }}</pre>
          </div>
        </div>
      </div>
      <div v-for="p in pending" :key="p.branch.id" class="flex flex-col">
        <div class="flex w-full min-w-0 items-center gap-2 px-2 py-1">
          <span class="flex size-3 flex-none items-center justify-center">
            <span class="dot-live size-1.5 rounded-full bg-brand" />
          </span>
          <span class="font-mono text-micro font-medium text-tool">agent</span>
          <span v-if="p.task" class="min-w-0 truncate font-mono text-micro text-ink-2">{{ p.task }}</span>
          <span class="ml-auto flex-none text-micro text-ink-3 tabular-nums" :title="t('common.running')">{{
            p.elapsed
          }}</span>
        </div>
        <Chip
          class="mt-0.5 mb-1.5 ml-[28px] self-start"
          icon="agent"
          icon-after="external"
          tone="agent"
          clickable
          :title="t('tools.row.subAgentTitle', { id: shortId(p.branch.id) })"
          @click="openAgentView(p.branch.id)"
        >
          {{ t('tools.row.openSubAgent') }}
        </Chip>
      </div>
    </div>
  </div>
</template>
