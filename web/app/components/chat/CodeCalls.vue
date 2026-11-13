<script setup lang="ts">
// Internal tool calls made by one `code` script: a compact list under the
// script body. Rows expand to their args/output; the hook trace of the
// enclosing call (tool.pre/post on internal calls) is shown underneath.
// Each invoke is an ordinary ToolResult node whose `invoke.parent` names the
// `code` call; they nest here instead of showing as chat rows.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Branch, Content, Decision, FilePatch, Tagged } from '../../lib/types'
import { openAgentView, state, useBranchView } from '../../lib/store'
import { argSummary, shortId } from '../../lib/util'
import { highlightLine } from '../../lib/markdown'
import { displayCall } from '../../lib/toolrows'
import { patchOfData } from '../../lib/diff'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import BashView from './BashView.vue'
import FileDiff from './FileDiff.vue'
import FileView from './FileView.vue'
import JsonTree from './JsonTree.vue'

const props = defineProps<{ parentCall: string }>()
const view = useBranchView()

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
}

const calls = computed<CodeCallRow[]>(() => {
  const out: CodeCallRow[] = []
  for (const n of view.nodes.value) {
    const b = n.body
    if (b.type !== 'tool_result' || !b.invoke || b.invoke.parent !== props.parentCall) continue
    out.push({
      call_id: b.call_id,
      ...displayCall({ name: b.invoke.name, args: b.invoke.args }),
      blocked: b.invoke.blocked ?? null,
      is_error: b.is_error,
      output: b.content.text,
      patch: b.content.data ?? null,
      dur_ms: n.dur_ms ?? 0,
    })
  }
  return out
})
const traces = computed(() => state.hookTrace.filter((e) => e.parent_call === props.parentCall))

/** Sub-agent branches forked by an internal call: keyed by its call id. */
const agentChildren = computed(() => {
  const map = new Map<string, Branch>()
  for (const b of Object.values(state.branches)) {
    if (b.kind.type === 'agent' && b.kind.via_plugin) map.set(b.kind.call_id, b)
  }
  return map
})
function openAgent(b: Branch): void {
  openAgentView(b.id)
}
const expanded = ref<Set<string>>(new Set())
const showTrace = ref(false)

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
/** A bash row is its own `$ command` line, whole once open. */
function commandOf(c: CodeCallRow): string {
  const cmd = argsOf(c)?.command
  return typeof cmd === 'string' ? cmd.trim() : ''
}

function effectLabel(e: Content): string {
  return e.data?.tag ? `note:${e.data.tag}` : 'note'
}
function decisionLabel(d: Decision): string | null {
  if (d.type === 'block') return `block: ${d.reason.slice(0, 40)}`
  if (d.type === 'rewrite') return 'rewrite'
  return null
}
function effectTone(e: Content): string {
  const tag = e.data?.tag
  if (tag?.startsWith('memory') || tag === 'card_use') return 'border-memory/35 bg-memory-soft text-memory'
  return 'border-line-soft bg-canvas text-ink-2'
}
function decisionTone(d: Decision): string {
  return d.type === 'block'
    ? 'border-danger/35 bg-danger-soft text-danger'
    : 'border-line-soft bg-canvas text-ink-2'
}
const iconTone: Record<Status, string> = {
  ok: 'text-tool',
  error: 'text-danger',
  blocked: 'text-danger',
}
</script>

<template>
  <div v-if="calls.length || traces.length" class="mt-2 flex flex-col gap-1.5">
    <div
      v-if="calls.length"
      class="flex flex-col divide-y divide-line-soft overflow-hidden rounded-control border border-line-soft"
    >
      <div
        v-for="c in calls"
        :key="c.call_id"
        class="flex flex-col"
        :class="{ 'bg-danger-soft/30': status(c) !== 'ok' }"
      >
        <button
          class="flex w-full min-w-0 gap-2 px-2 py-1 text-left transition-colors"
          :class="[
            c.name === 'bash' && isOpen(c.call_id) ? 'items-start' : 'items-center',
            isOpen(c.call_id) ? 'bg-surface-2' : 'hover:bg-surface-2/70',
          ]"
          :title="`${c.name} ${summary(c)} → ${statusLabel(c)}`"
          @click="toggle(c.call_id)"
        >
          <Icon
            :name="status(c) === 'ok' ? 'tool' : status(c) === 'blocked' ? 'blocked' : 'error'"
            :size="12"
            :class="iconTone[status(c)]"
          />
          <span
            class="font-mono text-micro font-medium"
            :class="status(c) === 'ok' ? 'text-tool' : 'text-danger'"
          >
            {{ c.name === 'bash' ? '$' : c.name }}
          </span>
          <span
            v-if="c.name === 'bash' && isOpen(c.call_id)"
            class="shell max-h-40 min-w-0 flex-1 overflow-y-auto font-mono text-micro break-words whitespace-pre-wrap text-ink"
            v-html="highlightLine(commandOf(c), 'bash')"
          ></span>
          <span v-else-if="summary(c)" class="min-w-0 truncate font-mono text-micro text-ink-2">{{
            summary(c)
          }}</span>
          <span class="ml-auto flex flex-none items-center gap-2">
            <span class="text-micro text-ink-3">{{ c.dur_ms }} ms</span>
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
          v-if="agentChildren.get(c.call_id)"
          class="mt-0.5 mb-1.5 ml-[28px] self-start"
          icon="agent"
          icon-after="external"
          tone="agent"
          clickable
          :title="t('tools.row.subAgentTitle', { id: shortId(agentChildren.get(c.call_id)!.id) })"
          @click="openAgent(agentChildren.get(c.call_id)!)"
        >
          {{ t('tools.row.openSubAgent') }}
        </Chip>
        <div v-if="isOpen(c.call_id)" class="reveal flex flex-col gap-1.5 pt-1 pr-2 pb-2 pl-[28px]">
          <FileDiff
            v-if="patchOf(c)"
            :patch="patchOf(c)"
            :path="pathOf(c)"
            :args="argsOf(c)"
            class="overflow-hidden rounded-control border border-line-soft"
          />
          <FileView
            v-else-if="c.name === 'read' && status(c) === 'ok'"
            :text="c.output"
            :path="pathOf(c)"
            :args="argsOf(c)"
            class="overflow-hidden rounded-control border border-line-soft"
          />
          <BashView
            v-else-if="c.name === 'bash'"
            :output="output(c)"
            :failed="status(c) !== 'ok'"
            class="overflow-hidden rounded-control border border-line-soft"
          />
          <template v-else>
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
          </template>
        </div>
      </div>
    </div>

    <button
      v-if="traces.length"
      class="flex items-center gap-1.5 self-start rounded-xs px-1.5 py-0.5 text-micro text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink-2"
      @click="showTrace = !showTrace"
    >
      <Icon name="chevron" :size="12" :class="{ 'rotate-90': showTrace }" />
      {{ t('tools.detail.hookTrace', { count: traces.length }) }}
    </button>
    <div v-if="showTrace" class="flex flex-col gap-1 pl-1">
      <div v-for="e in traces" :key="e.seq" class="flex flex-wrap items-center gap-1.5">
        <span class="font-mono text-micro text-tool">{{ e.event }}</span>
        <span class="text-micro text-ink-2">{{ e.plugin }}</span>
        <span class="font-mono text-micro text-ink-3">{{ e.hook_id }}</span>
        <span
          v-for="(a, i) in e.effects"
          :key="i"
          class="max-w-full truncate rounded-xs border px-1.5 font-mono text-micro"
          :class="effectTone(a)"
        >
          {{ effectLabel(a) }}
        </span>
        <span
          v-if="decisionLabel(e.decision)"
          class="max-w-full truncate rounded-xs border px-1.5 font-mono text-micro"
          :class="decisionTone(e.decision)"
        >
          {{ decisionLabel(e.decision) }}
        </span>
      </div>
    </div>
  </div>
</template>
