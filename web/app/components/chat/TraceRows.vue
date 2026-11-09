<script setup lang="ts">
// Rows of the trace view: one per node or hook firing, a sub-agent as a
// foldable group of its own rows. A row opens into its full JSON. Which rows
// are open and which kinds are hidden belong to TraceView, so they survive
// the rows being rebuilt as the session grows.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { TraceKind, TraceRow } from '../../lib/store'
import { toast } from '../../lib/state/ui'
import type { Node } from '../../lib/types'
import { copyText, fmtDuration, fmtTokens, preview, shortId } from '../../lib/util'
import Icon from '../ui/Icon.vue'
import JsonTree from './JsonTree.vue'

const props = defineProps<{
  rows: TraceRow[]
  open: Set<string>
  hidden: Set<TraceKind>
  /** call id → tool name, for naming results. */
  calls: Map<string, string>
}>()

function kindOf(row: TraceRow): TraceKind | null {
  if (row.kind === 'hook') return 'hook'
  if (row.kind === 'agent') return null
  const b = row.node.body
  if (b.type === 'tool_result') return 'tool'
  if (b.type === 'note') return b.content.text ? 'note' : 'marker'
  return b.type
}

const shown = computed(() =>
  props.rows.filter((r) => {
    const k = kindOf(r)
    return k === null || !props.hidden.has(k)
  }),
)

function toggle(key: string): void {
  if (props.open.has(key)) props.open.delete(key)
  else props.open.add(key)
}

/** Wall-clock time with milliseconds: rows land well under a second apart. */
function clock(ms: number): string {
  const d = new Date(ms)
  const p = (n: number, w = 2): string => String(n).padStart(w, '0')
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}.${p(d.getMilliseconds(), 3)}`
}

function label(row: TraceRow): string {
  if (row.kind === 'hook') return 'hook'
  if (row.kind === 'agent') return 'agent'
  const b = row.node.body
  if (b.type === 'note') return b.content.text ? 'note' : 'marker'
  return b.type
}

/** Literal utility strings (Tailwind only emits complete class names). */
function tone(row: TraceRow): string {
  if (row.kind === 'agent') return 'tone-agent'
  if (row.kind === 'hook') return row.hook.decision.type === 'block' ? 'tone-danger' : 'tone-neutral'
  const b = row.node.body
  if (b.type === 'user') return 'tone-brand'
  if (b.type === 'tool_result') return b.is_error ? 'tone-danger' : 'tone-tool'
  if (b.type === 'note') {
    const tag = b.content.data?.tag
    return tag?.startsWith('memory') || tag === 'card_use' ? 'tone-memory' : 'tone-neutral'
  }
  return 'tone-neutral'
}

function compact(v: unknown): string {
  return preview(typeof v === 'string' ? v : (JSON.stringify(v) ?? ''), 120)
}

function summary(row: TraceRow): string {
  if (row.kind === 'agent') {
    const k = row.branch.kind
    return k.type === 'agent' ? `${preview(k.task, 100)} · ${k.inherit.context} · ${row.branch.status}` : ''
  }
  if (row.kind === 'hook') {
    const h = row.hook
    const parts = [h.event, h.plugin]
    if (h.effects.length)
      parts.push(h.effects.map((e) => (e.data?.tag ? `note:${e.data.tag}` : 'note')).join(' '))
    if (h.decision.type === 'block') parts.push(`block: ${preview(h.decision.reason, 60)}`)
    else if (h.decision.type === 'rewrite') parts.push('rewrite')
    return parts.join(' · ')
  }
  const b = row.node.body
  switch (b.type) {
    case 'user': {
      const n = b.content.attachments?.length ?? 0
      return [preview(b.content.text, 120), n ? `+${n} img` : ''].filter(Boolean).join(' · ')
    }
    case 'assistant': {
      const u = b.usage
      const parts = [b.model, `${fmtTokens(u.prompt_tokens)} in · ${fmtTokens(u.completion_tokens)} out`]
      if (u.cached_tokens) parts.push(`${fmtTokens(u.cached_tokens)} cached`)
      if (b.tool_calls.length) parts.push(b.tool_calls.map((c) => c.name).join(', '))
      else if (b.content.text) parts.push(preview(b.content.text, 80))
      return parts.join(' · ')
    }
    case 'tool_result': {
      const name = b.invoke?.name ?? props.calls.get(b.call_id) ?? b.call_id
      return `${b.invoke ? '↳ ' : ''}${name} · ${preview(b.content.text, 100)}`
    }
    case 'note':
      if (!b.content.text)
        return b.content.data ? `${b.content.data.tag} · ${compact(b.content.data.data)}` : ''
      return [b.source, b.content.data?.tag, preview(b.content.text, 100)].filter(Boolean).join(' · ')
  }
  return ''
}

function at(row: TraceRow): number {
  if (row.kind === 'node') return row.node.at
  if (row.kind === 'hook') return row.hook.at
  return row.branch.created_at
}

function dur(row: TraceRow): string {
  const ms = row.kind === 'node' ? row.node.dur_ms : null
  return ms ? fmtDuration(ms) : ''
}

function id(row: TraceRow): string {
  if (row.kind === 'node') return shortId(row.node.id)
  if (row.kind === 'agent') return shortId(row.branch.id)
  return ''
}

function json(row: TraceRow): Node | object {
  if (row.kind === 'node') return row.node
  if (row.kind === 'hook') return row.hook
  return row.branch
}

async function copy(row: TraceRow): Promise<void> {
  const ok = await copyText(JSON.stringify(json(row), null, 2))
  toast('info', ok ? t('chat.trace.copied') : t('common.copyFailed'))
}
</script>

<template>
  <div class="flex min-w-0 flex-col">
    <div
      v-for="row in shown"
      :key="row.key"
      class="min-w-0"
      :data-node-id="row.kind === 'node' ? row.node.id : null"
    >
      <button
        type="button"
        class="flex w-full min-w-0 items-baseline gap-2 rounded-control px-2 py-1 text-left hover:bg-surface-2"
        :aria-expanded="open.has(row.key)"
        @click="toggle(row.key)"
      >
        <Icon
          name="chevron"
          :size="12"
          class="text-ink-3 transition-transform"
          :class="{ 'rotate-90': open.has(row.key) }"
        />
        <span class="hidden flex-none font-mono text-micro text-ink-3 tabular-nums sm:inline">{{
          clock(at(row))
        }}</span>
        <span class="w-12 flex-none text-right font-mono text-micro text-ink-3 tabular-nums">{{
          dur(row)
        }}</span>
        <span class="chip flex-none px-1.5 py-0 font-mono text-micro" :class="tone(row)">{{
          label(row)
        }}</span>
        <span class="min-w-0 flex-1 truncate text-meta text-ink-2">{{ summary(row) }}</span>
        <span class="hidden flex-none font-mono text-micro text-ink-3 md:inline">{{ id(row) }}</span>
      </button>
      <!-- a sub-agent opens into its own rows, hung off a rail in its tone -->
      <div
        v-if="row.kind === 'agent' && open.has(row.key)"
        class="mb-1 ml-3.5 border-l-2 border-agent/30 pl-2"
      >
        <TraceRows :rows="row.rows" :open="open" :hidden="hidden" :calls="calls" />
      </div>
      <div
        v-else-if="open.has(row.key)"
        class="group/json relative mt-0.5 mb-2 ml-6 overflow-x-auto rounded-card bg-canvas px-3 py-2"
      >
        <button
          type="button"
          class="icon-btn absolute top-1.5 right-1.5 opacity-0 group-hover/json:opacity-100 focus-visible:opacity-100"
          :title="t('chat.trace.copy')"
          :aria-label="t('chat.trace.copy')"
          @click="copy(row)"
        >
          <Icon name="copy" :size="14" />
        </button>
        <JsonTree :value="json(row)" />
      </div>
    </div>
  </div>
</template>
