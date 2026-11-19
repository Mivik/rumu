<script setup lang="ts">
// One "context" row: the Notes between messages. Recalled cards stay in view
// as card stubs ("recalled [A] [B]", clickable, opening the CardDrawer); every
// other notice (the default workspace, a loaded CLAUDE.md, a card that
// changed, a finished task, a plain note) is a side note, set on a divider
// across the column since it happens between the turns rather than in one.
// A few short ones sit on the divider as they are, "── ▣ loaded … ──"; more
// fold into "── 3 side notes › ──", opening to a list of sentences
// (NoticeRow) centred under it. Puts no node on the thread (only messages do).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Content, Node, TaskDone } from '../../lib/types'
import type { FilesReverted } from '../../lib/generated/FilesReverted'
import { CATALOG_TAG, ASSISTANT_INTRO_TAG, TASK_TAG, TASK_VERSION } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import { agentsOf, importOf, mountedId, updateOf, workspaceUseOf } from '../../lib/cards'
import { openDrawer } from '../../lib/store'
import { vEnter } from '../../lib/motion'
import NoticeRow from './NoticeRow.vue'
import type { Notice } from './NoticeRow.vue'

const props = defineProps<{ nodes: Node[] }>()

interface CardItem {
  node: Node
  cardId: string | null
  title: string
  hover: string
}

/** Decode a `kernel.task` completion payload, if that is what this note is. */
function taskDoneOf(content: Content): TaskDone | null {
  const d = content.data
  if (!d || d.tag !== TASK_TAG || d.v !== TASK_VERSION) return null
  const t = d.data as TaskDone
  return t && typeof t === 'object' && typeof t.id === 'string' ? t : null
}

/** Decode a `workspace.revert` marker, if that is what this note is. */
function revertOf(content: Content): FilesReverted | null {
  const d = content.data
  return d && d.tag === 'workspace.revert' && d.v === 1 ? (d.data as FilesReverted) : null
}

/** Up to 3 names shown; past that, 2 names + "+N" (rest in the hover title). */
const SHOWN = 3

const parsed = computed(() => {
  const cards: CardItem[] = []
  const notices = new Map<string, Notice>()
  const add = (n: Notice): void => {
    if (!notices.has(n.key)) notices.set(n.key, n)
  }

  for (const n of props.nodes) {
    const b = n.body
    if (b.type !== 'note') continue
    // A card catalog belongs to the activity run it precedes (CatalogList).
    if (b.content.data?.tag === CATALOG_TAG || b.content.data?.tag === ASSISTANT_INTRO_TAG) continue
    const task = taskDoneOf(b.content)
    const update = updateOf(b.content)
    const imp = importOf(b.content)
    const agents = agentsOf(b.content)
    const wsUse = workspaceUseOf(b.content)
    const revert = revertOf(b.content)
    if (task) {
      add({ kind: 'task', key: `task:${task.id}`, id: task.id, status: task.status, text: b.content.text })
    } else if (b.source === 'memory' && update) {
      const id = mountedId(update.deck, update.path) ?? update.path
      add({ kind: 'update', key: `update:${id}`, id, label: id, text: b.content.text })
    } else if (b.source === 'memory' && imp) {
      const cardId = mountedId(imp.deck, imp.path)
      const label = cardId ?? imp.path
      cards.push({
        node: n,
        cardId,
        title: label,
        hover: cardId ? `#${label}` : b.content.text,
      })
    } else if (agents) {
      // The injected text starts with `# CLAUDE.md (workspace `proj`)`;
      // name the document, not the directory basename.
      const doc = (b.content.text.split('\n')[0] ?? '')
        .replace(/^#\s*/, '')
        .replace(/\s*\(workspace `[^`]+`\)(\s*\(updated\))?\s*$/, '')
        .trim()
      const name = `${agents.ws}/${doc || agents.file}`
      add({ kind: 'agents', key: `agents:${name}`, name })
    } else if (wsUse) {
      add({ kind: 'workspace', key: `workspace:${wsUse.name}`, name: wsUse.name })
    } else if (revert) {
      const files = revert.files.map((f) => `${f.ws}:${f.path}`)
      add({ kind: 'revert', key: `revert:${n.id}`, files, text: files.join('\n') })
    } else if (b.content.text || !b.content.data) {
      add({ kind: 'note', key: `note:${n.id}`, text: b.content.text })
    }
    // An empty-text note with data that no producer claimed (e.g. the root
    // marker or a card_use record) has nothing to show here.
  }
  return { cards, notices: [...notices.values()] }
})

const cards = computed(() => parsed.value.cards)
const notices = computed(() => parsed.value.notices)
const cardsShown = computed(() => cards.value.slice(0, cards.value.length > SHOWN ? 2 : SHOWN))
const cardsMore = computed(() => (cards.value.length > SHOWN ? cards.value.length - 2 : 0))
const cardsTitle = computed(() => cards.value.map((c) => c.hover).join('\n'))

function openCard(c: CardItem): void {
  if (c.cardId) openDrawer(c.cardId, c.node.id)
}

/** Up to this many side notes sit on the divider unfolded, one per line. */
const INLINE = 2

/** A note that reads on one line; a long or multi-line one stays folded. */
const short = (n: Notice): boolean => n.kind !== 'note' || (n.text.length <= 80 && !n.text.includes('\n'))

const inline = computed(() => notices.value.length <= INLINE && notices.value.every(short))

const open = ref(false)
</script>

<template>
  <div class="flex min-w-0 flex-col items-start gap-2">
    <div v-if="cards.length" class="flex max-w-full min-w-0 text-meta">
      <!-- recalled cards: "recalled" then each card as a little card stub (+N) -->
      <span
        v-enter="'enter-recall'"
        class="inline-flex min-w-0 max-w-full flex-wrap items-baseline gap-1.5"
        :title="cardsTitle"
      >
        <!-- set like the fold lines (.reason-toggle); the row sits on the
             titles' baseline, which are larger and in the card face -->
        <span class="inline-flex items-baseline gap-1.5 text-micro font-semibold text-ink-3">
          <Icon name="memory" :size="12" class="text-memory" />
          {{ t('cards.pills.imported') }}
        </span>
        <template v-for="(c, i) in cardsShown" :key="c.node.id">
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
        <span v-if="cardsMore" class="text-ink-3">+{{ cardsMore }}</span>
      </span>
    </div>

    <!-- every other notice: a divider across the column, between the turns -->
    <template v-if="inline">
      <div v-for="n in notices" :key="n.key" v-enter="'enter-fade'" class="flex w-full items-center gap-3">
        <span class="h-px min-w-6 flex-1 bg-line" />
        <NoticeRow :notice="n" />
        <span class="h-px min-w-6 flex-1 bg-line" />
      </div>
    </template>
    <template v-else-if="notices.length">
      <div v-enter="'enter-fade'" class="flex w-full items-center gap-3">
        <span class="h-px flex-1 bg-line" />
        <button
          type="button"
          class="reason-toggle flex-none"
          :class="open ? 'text-ink-2' : 'text-ink-3 hover:text-ink-2'"
          :aria-expanded="open"
          @click="open = !open"
        >
          <span>{{ t('cards.notes.count', { count: notices.length }) }}</span>
          <Icon name="chevron" :size="12" class="reason-chevron flex-none" :class="{ 'rotate-90': open }" />
        </button>
        <span class="h-px flex-1 bg-line" />
      </div>
      <div v-if="open" class="reveal mx-auto flex flex-col items-start gap-1">
        <NoticeRow v-for="n in notices" :key="n.key" :notice="n" />
      </div>
    </template>
  </div>
</template>
