<script setup lang="ts">
// One "context" row: consecutive Notes/Markers collapsed into compact,
// tinted pills — "recalled [A] [B]" as card stubs, "card A changed". Same-kind
// items share one pill; card names are clickable and open the CardDrawer.
// Each pill carries node ids so the Context panel can jump to the injection
// point. Pills pop in when they happen live; a recalled memory fades in and
// underlines its cards one by one. It lines up with the content column's left
// edge and puts no node on the thread (only messages do).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Content, Node, TaskDone, TaskStatus } from '../../lib/types'
import { CATALOG_TAG, ASSISTANT_INTRO_TAG, TASK_TAG, TASK_VERSION } from '../../lib/types'
import Chip from '../ui/Chip.vue'
import Icon from '../ui/Icon.vue'
import {
  agentsOf,
  cardByKey,
  cardForImport,
  cardLabel,
  importOf,
  updateOf,
  workspaceUseOf,
} from '../../lib/cards'
import { preview } from '../../lib/util'
import { openDrawer, openTaskDetail } from '../../lib/store'
import { taskStatusWord } from '../../lib/tasks'
import { vEnter } from '../../lib/motion'

const props = defineProps<{ nodes: Node[] }>()

interface CardItem {
  node: Node
  cardId: string | null
  title: string
  hover: string
}

/** A card that opens by its full id, shown under its mount-free label. */
interface CardRef {
  id: string
  label: string
}

/** Decode a `kernel.task` completion payload, if that is what this note is. */
function taskDoneOf(content: Content): TaskDone | null {
  const d = content.data
  if (!d || d.tag !== TASK_TAG || d.v !== TASK_VERSION) return null
  const t = d.data as TaskDone
  return t && typeof t === 'object' && typeof t.id === 'string' ? t : null
}

interface PillSpec {
  key: string
  kind: 'memory' | 'update' | 'workspace' | 'agents' | 'note' | 'task'
  title: string
  cards?: CardItem[]
  refs?: CardRef[]
  names?: string[]
  texts?: string[]
  expandable?: boolean
  taskId?: string
  taskStatus?: TaskStatus
}

/** Up to 3 names shown; past that, 2 names + "+N" (rest in the hover title). */
const SHOWN = 3

function memoryShown(p: PillSpec): CardItem[] {
  const cards = p.cards ?? []
  return cards.slice(0, cards.length > SHOWN ? 2 : SHOWN)
}

function memoryMore(p: PillSpec): number {
  const cards = p.cards ?? []
  return cards.length > SHOWN ? cards.length - 2 : 0
}

function openCard(c: CardItem): void {
  if (c.cardId) openDrawer(c.cardId, c.node.id)
}

const pills = computed<PillSpec[]>(() => {
  const byKind = new Map<string, PillSpec>()

  const ensure = (
    key: string,
    kind: PillSpec['kind'],
    title: string,
    extra: Partial<PillSpec> = {},
  ): PillSpec => {
    let p = byKind.get(key)
    if (!p) {
      p = { key, kind, title, ...extra }
      byKind.set(key, p)
    }
    return p
  }

  for (const n of props.nodes) {
    const b = n.body
    // A card catalog belongs to the activity run it precedes (CatalogList).
    if (b.type === 'note' && b.content.data?.tag === CATALOG_TAG) continue
    if (b.type === 'note' && b.content.data?.tag === ASSISTANT_INTRO_TAG) continue
    if (b.type === 'note') {
      const task = taskDoneOf(b.content)
      const update = updateOf(b.content)
      const imp = importOf(b.content)
      const agents = agentsOf(b.content)
      const wsUse = workspaceUseOf(b.content)
      if (task) {
        // A background-task completion: one pill per task, opening the live /
        // buffered output in the tasks panel.
        ensure(`task:${task.id}`, 'task', b.content.text, {
          taskId: task.id,
          taskStatus: task.status,
        })
      } else if (b.source === 'memory' && update) {
        // Update notice: names the card, carries no body.
        const p = ensure('update', 'update', t('cards.pills.changed'), { refs: [], texts: [] })
        const key = `${update.deck}:${update.path}`
        const id = cardByKey.value.get(key)?.id ?? update.path
        if (!p.refs!.some((r) => r.id === id)) p.refs!.push({ id, label: cardLabel(id) })
        p.texts!.push(b.content.text)
      } else if (b.source === 'memory' && imp) {
        const p = ensure('memory', 'memory', t('cards.pills.imported'), { cards: [] })
        const card = cardForImport(imp)
        if (card) {
          const label = cardLabel(card.id)
          p.cards!.push({
            node: n,
            cardId: card.id,
            title: label,
            hover: `#${label}${card.description ? ` — ${card.description}` : ''}`,
          })
        } else {
          p.cards!.push({
            node: n,
            cardId: null,
            title: imp.path,
            hover: b.content.text,
          })
        }
      } else if (agents) {
        // The injected text starts with `# CLAUDE.md (workspace `proj`)`;
        // label the pill with that document, not with the directory basename.
        const first = b.content.text.split('\n')[0] ?? ''
        const doc = first
          .replace(/^#\s*/, '')
          .replace(/\s*\(workspace `[^`]+`\)(\s*\(updated\))?\s*$/, '')
          .trim()
        const name = `${agents.ws}/${doc || agents.file}`
        const p = ensure('agents', 'agents', '', { names: [] })
        if (!p.names!.includes(name)) p.names!.push(name)
      } else if (wsUse) {
        const p = ensure('workspace', 'workspace', '', { names: [] })
        if (!p.names!.includes(wsUse.name)) p.names!.push(wsUse.name)
      } else if (b.content.text || !b.content.data) {
        const p = ensure(`note:${b.source}`, 'note', b.content.text, {
          texts: [],
          expandable: true,
        })
        p.texts!.push(b.content.text)
      }
      // An empty-text note with data that no producer claimed (e.g. the root
      // marker or a card_use record) has nothing to show here.
    }
  }

  const out: PillSpec[] = []
  if (byKind.has('memory')) {
    const p = byKind.get('memory')!
    p.title = p.cards!.map((c) => c.hover).join('\n')
    out.push(p)
  }
  for (const p of byKind.values()) {
    if (p.kind === 'memory') continue
    if (p.kind === 'note') p.title = p.texts!.join('\n\n')
    if (p.kind === 'agents') p.title = p.names!.join('\n')
    if (p.kind === 'update') p.title = p.texts!.join('\n\n')
    out.push(p)
  }
  return out
})

const expandedId = ref<string | null>(null)
const expandedPill = computed(() => pills.value.find((x) => x.key === expandedId.value) ?? null)
const expandedText = computed(() => {
  const p = expandedPill.value
  return p?.expandable ? (p.texts ?? []).join('\n\n') : ''
})

function toggle(p: PillSpec): void {
  if (!p.expandable) return
  expandedId.value = expandedId.value === p.key ? null : p.key
}

function label(names: string[]): string {
  return names.join(', ')
}
</script>

<template>
  <div class="relative flex flex-wrap items-center gap-1.5 text-meta">
    <span
      v-for="p in pills"
      :key="p.key"
      v-enter="p.kind === 'memory' ? 'enter-recall' : 'enter-pop'"
      class="inline-flex min-w-0 max-w-full"
      :class="p.kind === 'memory' ? 'rounded-control' : 'rounded-full'"
    >
      <!-- memory: "imported" then each card as a little card stub (+N) -->
      <span
        v-if="p.kind === 'memory'"
        class="inline-flex min-w-0 max-w-full flex-wrap items-baseline gap-1.5"
        :title="p.title"
      >
        <!-- set like the fold lines around it (.reason-toggle); the row sits
             on the titles' baseline, which are larger and in the card face -->
        <span class="inline-flex items-baseline gap-1.5 text-micro font-semibold text-ink-3">
          <Icon name="memory" :size="12" class="text-memory" />
          {{ t('cards.pills.imported') }}
        </span>
        <template v-for="(c, i) in memoryShown(p)" :key="c.node.id">
          <button
            v-if="c.cardId"
            class="card-ref"
            :style="{ '--i': i }"
            :title="c.hover"
            type="button"
            @click.stop="openCard(c)"
          >
            <span class="truncate">{{ c.title }}</span>
          </button>
          <span v-else class="card-ref" :style="{ '--i': i }" :title="c.hover">
            <span class="truncate">{{ c.title }}</span>
          </span>
        </template>
        <span v-if="memoryMore(p)" class="text-ink-3">+{{ memoryMore(p) }}</span>
      </span>

      <!-- update notices: "A changed (+N)", click opens the card -->
      <Chip v-else-if="p.kind === 'update'" icon="note" tone="memory" :title="p.title">
        <template v-for="(ref, i) in p.refs ?? []" :key="ref.id">
          <span v-if="i > 0">,</span>
          <button class="card-ref" type="button" @click.stop="openDrawer(ref.id)">
            {{ ref.label }}
          </button>
        </template>
        <span class="text-ink-3">&nbsp;{{ t('cards.pills.changed') }}</span>
      </Chip>

      <Chip
        v-else-if="p.kind === 'workspace'"
        icon="mount"
        tone="tool"
        :label="t('cards.pills.workspace', { names: label(p.names ?? []) })"
      />
      <Chip
        v-else-if="p.kind === 'agents'"
        icon="file"
        tone="tool"
        :label="label(p.names ?? [])"
        :title="p.title"
      />
      <Chip
        v-else-if="p.kind === 'task'"
        icon="tool"
        tone="tool"
        clickable
        :label="t('cards.pills.task', { id: p.taskId, status: taskStatusWord(p.taskStatus ?? 'done') })"
        :title="p.title"
        @click="openTaskDetail(p.taskId!)"
      />
      <Chip
        v-else
        icon="note"
        tone="neutral"
        :label="preview(p.texts?.[0] ?? '', 40)"
        :title="p.title"
        clickable
        @click="toggle(p)"
      />
    </span>
    <div
      v-if="expandedText"
      class="reveal mt-0.5 basis-full whitespace-pre-wrap rounded-control bg-canvas px-3 py-2 text-meta text-ink-2"
    >
      {{ expandedText }}
    </div>
  </div>
</template>
