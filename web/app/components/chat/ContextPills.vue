<script setup lang="ts">
// One "context" row: consecutive Notes/Markers collapsed into compact,
// tinted pills — "imported A, B", "card catalog (+N)", "reverted · lesson",
// "card A changed". Same-kind items share one pill; card names are clickable
// and open the CardDrawer. Each pill carries node ids so the Context panel can
// jump to the injection point. Pills pop in when they happen live; a recalled
// memory glows once. It lines up with the content column's left edge and puts
// no node on the thread (only messages do).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { CatalogNote, Content, Node, TaskDone, TaskStatus } from '../../lib/types'
import { CATALOG_TAG, CATALOG_VERSION, TASK_TAG, TASK_VERSION } from '../../lib/types'
import Chip from '../ui/Chip.vue'
import {
  agentsOf,
  revertOf,
  cardForImport,
  importOf,
  noteTitleFromText,
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

interface CatalogItem {
  node: Node
  catalog: CatalogNote
}

/** Decode a `memory.catalog` note payload, if that is what this node is. */
function catalogOf(n: Node): CatalogNote | null {
  if (n.body.type !== 'note') return null
  const d = n.body.content.data
  if (!d || d.tag !== CATALOG_TAG || d.v !== CATALOG_VERSION) return null
  const c = d.data as CatalogNote
  return c && typeof c === 'object' && c.scope && Array.isArray(c.entries) ? c : null
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
  kind: 'memory' | 'update' | 'catalog' | 'workspace' | 'agents' | 'revert' | 'note' | 'task'
  nodeIds: string[]
  title: string
  cards?: CardItem[]
  catalogs?: CatalogItem[]
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
      p = { key, kind, nodeIds: [], title, ...extra }
      byKind.set(key, p)
    }
    return p
  }

  const addNode = (p: PillSpec, n: Node): void => {
    p.nodeIds.push(n.id)
  }

  for (const n of props.nodes) {
    const b = n.body
    const catalog = catalogOf(n)
    if (catalog) {
      const p = ensure('catalog', 'catalog', t('cards.pills.catalog'), {
        catalogs: [],
        expandable: true,
      })
      addNode(p, n)
      p.catalogs!.push({ node: n, catalog })
    } else if (b.type === 'note') {
      const task = taskDoneOf(b.content)
      const update = updateOf(b.content)
      const imp = importOf(b.content)
      const agents = agentsOf(b.content)
      const wsUse = workspaceUseOf(b.content)
      const revert = b.source === 'system' ? revertOf(b.content) : null
      if (task) {
        // A background-task completion: one pill per task, opening the live /
        // buffered output in the tasks panel.
        const p = ensure(`task:${task.id}`, 'task', b.content.text, {
          taskId: task.id,
          taskStatus: task.status,
        })
        addNode(p, n)
      } else if (b.source === 'memory' && update) {
        // Update notice: names the card, carries no body.
        const p = ensure('update', 'update', t('cards.pills.changed'), { names: [], texts: [] })
        addNode(p, n)
        if (!p.names!.includes(update.id)) p.names!.push(update.id)
        p.texts!.push(b.content.text)
      } else if (b.source === 'memory' && imp) {
        const p = ensure('memory', 'memory', t('cards.pills.imported'), { cards: [] })
        addNode(p, n)
        const card = cardForImport(imp)
        if (card) {
          p.cards!.push({
            node: n,
            cardId: card.id,
            title: card.title || card.id,
            hover: `#${card.id}${card.description ? ` — ${card.description}` : ''}`,
          })
        } else {
          p.cards!.push({
            node: n,
            cardId: imp.id,
            title: noteTitleFromText(b.content.text) || imp.id,
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
        addNode(p, n)
        if (!p.names!.includes(name)) p.names!.push(name)
      } else if (revert) {
        const p = ensure(`revert:${n.id}`, 'revert', revert.lesson, { texts: [] })
        addNode(p, n)
        p.texts!.push(revert.lesson)
      } else if (wsUse) {
        const p = ensure('workspace', 'workspace', '', { names: [] })
        addNode(p, n)
        if (!p.names!.includes(wsUse.name)) p.names!.push(wsUse.name)
      } else if (b.content.text || !b.content.data) {
        const p = ensure(`note:${b.source}`, 'note', b.content.text, {
          texts: [],
          expandable: true,
        })
        addNode(p, n)
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
const expandedCatalogs = computed(() => expandedPill.value?.catalogs ?? [])
const showCatalog = computed(() => expandedPill.value?.kind === 'catalog')
const expandedText = computed(() => {
  const p = expandedPill.value
  return p?.expandable && p.kind !== 'catalog' ? (p.texts ?? []).join('\n\n') : ''
})

function toggle(p: PillSpec): void {
  if (!p.expandable) return
  expandedId.value = expandedId.value === p.key ? null : p.key
}

/** Catalog label: `card catalog (+N)` when dir/ws notes follow the root. */
function catalogLabel(p: PillSpec): string {
  const extra = (p.catalogs ?? []).filter((c) => c.catalog.scope.kind !== 'root').length
  return extra > 0 ? t('cards.pills.catalogMore', { count: extra }) : t('cards.pills.catalog')
}

function catalogTitle(c: CatalogNote): string {
  switch (c.scope.kind) {
    case 'root':
      return t('cards.pills.catalogTitles.root')
    case 'directory':
      return t('cards.pills.catalogTitles.directory', { prefix: c.scope.prefix })
    case 'workspace':
      return t('cards.pills.catalogTitles.workspace', { name: c.scope.name })
  }
}

function foldedLabel(names: string[]): string {
  const shown = names.slice(0, 5).join(', ')
  return t('cards.pills.moreCards', {
    count: names.length,
    names: `${shown}${names.length > 5 ? '…' : ''}`,
  })
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
      v-enter="p.kind === 'memory' ? 'enter-glow' : 'enter-pop'"
      :data-node-id="p.nodeIds[0]"
      :data-node-ids="p.nodeIds.join(' ')"
      class="inline-flex min-w-0 max-w-full rounded-full"
    >
      <!-- memory: "imported A, B (+N)" with clickable card names -->
      <Chip v-if="p.kind === 'memory'" icon="memory" tone="memory" :title="p.title">
        {{ t('cards.pills.imported') }}
        <template v-for="(c, i) in memoryShown(p)" :key="c.node.id">
          <span v-if="i > 0" class="mr-1">,</span>
          <button
            v-if="c.cardId"
            class="font-semibold underline-offset-2 hover:underline"
            :title="c.hover"
            type="button"
            @click.stop="openCard(c)"
          >
            {{ c.title }}
          </button>
          <span v-else :title="c.hover">{{ c.title }}</span>
        </template>
        <span v-if="memoryMore(p)" class="text-ink-3"> +{{ memoryMore(p) }}</span>
      </Chip>

      <!-- update notices: "A changed (+N)", click opens the card -->
      <Chip v-else-if="p.kind === 'update'" icon="note" tone="memory" :title="p.title">
        <template v-for="(name, i) in p.names ?? []" :key="name">
          <span v-if="i > 0">,</span>
          <button
            class="font-semibold underline-offset-2 hover:underline"
            type="button"
            @click.stop="openDrawer(name)"
          >
            {{ name }}
          </button>
        </template>
        <span class="text-ink-3">&nbsp;{{ t('cards.pills.changed') }}</span>
      </Chip>

      <Chip
        v-else-if="p.kind === 'catalog'"
        icon="memory"
        tone="memory"
        :label="catalogLabel(p)"
        :title="p.title"
        clickable
        @click="toggle(p)"
      />
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
        v-else-if="p.kind === 'revert'"
        icon="revert"
        tone="revert"
        :label="t('cards.pills.reverted', { text: preview(p.texts?.[0] ?? '', 40) })"
        :title="p.title"
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
      v-if="showCatalog"
      class="reveal mt-0.5 max-h-72 basis-full overflow-y-auto rounded-control bg-canvas px-3 py-2 text-meta text-ink-2"
    >
      <div v-for="c in expandedCatalogs" :key="c.node.id" class="mt-2 first:mt-0">
        <div class="font-semibold text-ink">{{ catalogTitle(c.catalog) }}</div>
        <ul class="mt-1 flex flex-col gap-0.5">
          <li v-for="e in c.catalog.entries" :key="e.id">
            <button
              type="button"
              class="font-semibold text-brand-strong underline-offset-2 hover:underline"
              @click="openDrawer(e.id)"
            >
              {{ e.title }}
            </button>
            <span v-if="e.summary" class="text-ink-3"> — {{ e.summary }}</span>
          </li>
        </ul>
        <div v-if="c.catalog.folded.length" class="mt-1 flex flex-col gap-0.5 text-ink-3">
          <div v-for="f in c.catalog.folded" :key="f.prefix">
            <span class="font-mono">{{ f.prefix }}/</span> — {{ foldedLabel(f.names) }}
          </div>
        </div>
      </div>
    </div>
    <div
      v-else-if="expandedText"
      class="reveal mt-0.5 basis-full whitespace-pre-wrap rounded-control bg-canvas px-3 py-2 text-meta text-ink-2"
    >
      {{ expandedText }}
    </div>
  </div>
</template>
