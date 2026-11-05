<script setup lang="ts">
// Internal tool calls made by one `code` script: a compact list under the
// script body. Rows expand to their args/output; the hook trace of the
// enclosing call (tool.pre/post on internal calls) is shown underneath.
// Internal calls are trace data, not nodes — they survive a reload via
// session.tree `code_calls`.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Branch, CodeCall, Content, Decision, FilePatch } from '../../lib/types'
import { selectBranch, state } from '../../lib/store'
import { argSummary, shortId } from '../../lib/util'
import { prettyJson } from '../../lib/markdown'
import { patchOfData } from '../../lib/diff'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import FileDiff from './FileDiff.vue'

const props = defineProps<{ parentCall: string }>()

const calls = computed<CodeCall[]>(() => state.codeCalls[props.parentCall] ?? [])
const traces = computed(() => state.hookTrace.filter((e) => e.parent_call === props.parentCall))

/** Sub-agent branches forked by an internal call: `call_id = "{parent}.{seq}"`. */
const agentChildren = computed(() => {
  const map = new Map<number, Branch>()
  const prefix = `${props.parentCall}.`
  for (const b of Object.values(state.branches)) {
    if (b.kind.type !== 'agent' || !b.kind.via_plugin) continue
    if (!b.kind.call_id.startsWith(prefix)) continue
    const seq = Number(b.kind.call_id.slice(prefix.length))
    if (Number.isFinite(seq)) map.set(seq, b)
  }
  return map
})
function openAgent(b: Branch): void {
  void selectBranch(b.id)
}
const expanded = ref<Set<number>>(new Set())
const showTrace = ref(false)

type Status = 'ok' | 'error' | 'blocked'

function status(c: CodeCall): Status {
  return c.blocked ? 'blocked' : c.is_error ? 'error' : 'ok'
}
function statusLabel(c: CodeCall): string {
  return t(`tools.row.status.${status(c)}`)
}
function isOpen(seq: number): boolean {
  return expanded.value.has(seq)
}
function toggle(seq: number): void {
  if (expanded.value.has(seq)) expanded.value.delete(seq)
  else expanded.value.add(seq)
}
function summary(c: CodeCall): string {
  return argSummary({ name: c.name, args: c.args })
}
function output(c: CodeCall): string {
  return c.blocked ?? c.output
}

/** A successful write/edit expands to its diff, like a direct call's row. */
function patchOf(c: CodeCall): FilePatch | null {
  return status(c) === 'ok' ? patchOfData(c.patch) : null
}
function argsOf(c: CodeCall): Record<string, unknown> | null {
  const a = c.args
  return a && typeof a === 'object' && !Array.isArray(a) ? (a as Record<string, unknown>) : null
}
function pathOf(c: CodeCall): string {
  const p = argsOf(c)?.path
  return typeof p === 'string' ? p : ''
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
        :key="c.seq"
        class="flex flex-col"
        :class="{ 'bg-danger-soft/30': status(c) !== 'ok' }"
      >
        <button
          class="flex w-full min-w-0 items-center gap-2 px-2 py-1 text-left transition-colors hover:bg-surface-2/70"
          :title="`${c.name} ${summary(c)} → ${statusLabel(c)}`"
          @click="toggle(c.seq)"
        >
          <Icon
            :name="status(c) === 'ok' ? 'tool' : status(c) === 'blocked' ? 'blocked' : 'error'"
            :size="11"
            :class="iconTone[status(c)]"
          />
          <span
            class="font-mono text-micro font-medium"
            :class="status(c) === 'ok' ? 'text-tool' : 'text-danger'"
          >
            {{ c.name }}
          </span>
          <span v-if="summary(c)" class="min-w-0 truncate font-mono text-micro text-ink-2">{{
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
              :size="10"
              class="text-ink-3/70 transition-transform duration-100"
              :class="{ 'rotate-90': isOpen(c.seq) }"
            />
          </span>
        </button>
        <Chip
          v-if="agentChildren.get(c.seq)"
          class="mt-0.5 mb-1.5 ml-[28px] self-start"
          icon="agent"
          icon-after="external"
          tone="agent"
          clickable
          :title="t('tools.row.subAgentTitle', { id: shortId(agentChildren.get(c.seq)!.id) })"
          @click="openAgent(agentChildren.get(c.seq)!)"
        >
          {{ t('tools.row.openSubAgent') }}
        </Chip>
        <div v-if="isOpen(c.seq)" class="reveal flex flex-col gap-1.5 pt-1 pr-2 pb-2 pl-[28px]">
          <FileDiff
            v-if="patchOf(c)"
            :patch="patchOf(c)"
            :path="pathOf(c)"
            :args="argsOf(c)"
            class="overflow-hidden rounded-control border border-line-soft"
          />
          <template v-else>
            <div>
              <div class="mb-1 text-micro font-semibold text-ink-3">{{ t('tools.detail.args') }}</div>
              <pre class="code">{{ prettyJson(c.args) }}</pre>
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
      <Icon name="chevron" :size="9" :class="{ 'rotate-90': showTrace }" />
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
