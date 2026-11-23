<script setup lang="ts">
// One "context" row: the Notes between messages. Recalled cards stay in view
// as card stubs ("recalled [A] [B]", clickable, opening the CardDrawer); every
// other notice (the default workspace, the user moving the session into or
// out of a project, a loaded CLAUDE.md, a card that changed, a finished task,
// a plain note) is a side note, set on a divider
// across the column since it happens between the turns rather than in one.
// A few short ones sit on the divider as they are, "── ▣ loaded … ──"; more
// fold into "── 3 side notes › ──", opening to a list of sentences
// (NoticeRow) centred under it. Puts no node on the thread (only messages do).
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { Content, Node, TaskDone } from '../../lib/types'
import type { FilesReverted } from '../../lib/generated/FilesReverted'
import type { McpIntro } from '../../lib/generated/McpIntro'
import type { McpAppContext } from '../../lib/generated/McpAppContext'
import type { SkillsIntro } from '../../lib/generated/SkillsIntro'
import type { WatchEvent } from '../../lib/generated/WatchEvent'
import type { WatchSilence } from '../../lib/generated/WatchSilence'
import type { AgentReply } from '../../lib/generated/AgentReply'
import type { CardMount } from '../../lib/generated/CardMount'
import { CATALOG_TAG, ASSISTANT_INTRO_TAG, TASK_TAG, TASK_VERSION } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import { agentsOf, importOf, mountedId, updateOf, workspaceUseOf } from '../../lib/cards'
import { openDrawer, state } from '../../lib/store'
import { taskLabel } from '../../lib/tasks'
import { vEnter } from '../../lib/motion'
import { rendererOf } from '../../lib/plugins'
import NoticeRow from './NoticeRow.vue'
import type { Notice } from './NoticeRow.vue'

const props = defineProps<{ nodes: Node[] }>()

interface CardItem {
  node: Node
  cardId: string | null
  title: string
  hover: string
}

/** Decode a `task.done` completion payload, if that is what this note is. */
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

/** The payload of a note of `tag` (v1), if that is what this note is. */
function dataOf<T>(content: Content, tag: string): T | null {
  const d = content.data
  return d && d.tag === tag && d.v === 1 ? (d.data as T) : null
}

/** `bash: cargo test` for a task: from the open session's task list, else
 *  what the completion payload says, else its id. */
function labelOf(id: string, kind?: TaskDone['kind']): string {
  const tk = state.tasks[id]
  if (tk) return taskLabel(tk)
  if (kind?.type === 'tool') return kind.args_summary ? `${kind.name}: ${kind.args_summary}` : kind.name
  return id
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
    const mcp = dataOf<McpIntro>(b.content, 'mcp.intro')
    const appContext = dataOf<McpAppContext>(b.content, 'mcp.app_context')
    const skills = dataOf<SkillsIntro>(b.content, 'skills.intro')
    const watch = dataOf<WatchEvent>(b.content, 'workspace.watch')
    const silence = dataOf<WatchSilence>(b.content, 'workspace.watch_silence')
    const reply = dataOf<AgentReply>(b.content, 'agent.reply')
    // The user moving the session (the model's own mounts are pure markers,
    // shown by their tool rows).
    const entered = b.content.text ? dataOf<CardMount>(b.content, 'card.mount') : null
    const left = b.content.text ? dataOf<{ at: string }>(b.content, 'card.unmount') : null
    if (entered?.project || left) {
      const name = entered
        ? (state.projects.find((p) => p.id === entered.deck)?.name ?? entered.deck.slice(0, 8))
        : null
      add({ kind: 'project', key: `project:${n.id}`, name, text: b.content.text })
    } else if (task) {
      const label = labelOf(task.id, task.kind)
      add({
        kind: 'task',
        key: `task:${task.id}`,
        id: task.id,
        label,
        status: task.status,
        text: b.content.text,
      })
    } else if (mcp) {
      const servers = mcp.servers.map((name) => ({ name, ...mcp.status?.[name] }))
      add({
        kind: 'mcp',
        key: `mcp:${n.id}`,
        servers,
        text: servers.map((s) => (s.error ? `${s.name}: ${s.error}` : s.name)).join('\n'),
      })
    } else if (appContext) {
      const tool = `${appContext.server}/${appContext.tool}`
      add({ kind: 'app', key: `app:${n.id}`, tool, text: b.content.text })
    } else if (skills) {
      const text = skills.skills.map((s) => (skills.about?.[s] ? `${s}: ${skills.about[s]}` : s)).join('\n')
      add({ kind: 'skills', key: `skills:${n.id}`, names: skills.skills, text })
    } else if (watch) {
      add({
        kind: 'watch',
        key: `watch:${n.id}`,
        id: watch.task,
        label: labelOf(watch.task),
        count: watch.count ?? 0,
        matching: watch.matching ?? false,
        text: b.content.text,
      })
    } else if (silence) {
      add({
        kind: 'silence',
        key: `silence:${n.id}`,
        id: silence.task,
        label: labelOf(silence.task),
        secs: silence.secs,
        matching: silence.matching,
      })
    } else if (reply) {
      // The text after the `sub-agent "…" replied:` line is the reply.
      const text = b.content.text.slice(b.content.text.indexOf('\n') + 1)
      add({ kind: 'reply', key: `reply:${n.id}`, branch: reply.branch, label: reply.label, text })
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
    } else if (rendererOf(b.content.data)) {
      add({ kind: 'plugin', key: `plugin:${n.id}`, data: b.content.data!, text: b.content.text })
    } else if (b.content.text || !b.content.data) {
      // Name the plugin a note comes from; rumu's own (`system`) go unnamed.
      const source = b.source === 'system' ? undefined : b.source
      add({ kind: 'note', key: `note:${n.id}`, source, text: b.content.text })
    }
    // An empty-text note with data that no producer claimed (e.g. the root
    // marker or a card.use record) has nothing to show here.
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
