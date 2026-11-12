<script setup lang="ts">
// The work between two pieces of an assistant turn's visible text — the cards
// recalled for it, the card catalog handed to the model, each step's reasoning
// and its tool calls. While it is the turn's frontier (`current`) it stays
// open, every call a row as it lands, the running ones last with their
// elapsed time and live output (RunningTools). Once the model moves on — its
// answer starts, the turn ends — it folds into one quiet line of what it
// took:
//   Worked for 3m 12s · 27 calls · 1 error [blocked] ›
// (a lone call names itself instead: "read store.ts"). Folded or not, what
// the run changed stands outside the fold: each edited file as an EditCard,
// and the sub-agents it started as AgentCards. Recall and catalog go with
// every turn, so neither shows on the line; a run without calls names its
// thinking ("Thought for 2s"), or its recall / catalog when that is all.
// Opened, the steps show in order under a hairline: the catalog's cards,
// each thought, each run of calls as a ToolList box, each edit as its card.
// A bare thought — no catalog, no calls — keeps its own disclosure
// (ThoughtStep).
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { t, locale } from '../../i18n'
import type {
  ActivityRecall,
  ActivityRunning,
  ActivityStep,
  ActivityThought,
  ToolRowInfo,
} from '../../lib/store'
import { useBranchView } from '../../lib/store'
import type { Node } from '../../lib/types'
import type { PhrasePart, ToolRowView } from '../../lib/toolrows'
import { isFileEdit, toolPhrase, toolRowTitle, toolRowView } from '../../lib/toolrows'
import { contentText, fmtThought } from '../../lib/util'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import AgentCard from './AgentCard.vue'
import CatalogList from './CatalogList.vue'
import EditCard from './EditCard.vue'
import RecallList from './RecallList.vue'
import RunningTools from './RunningTools.vue'
import ThoughtStep from './ThoughtStep.vue'
import ToolList from './ToolList.vue'

const props = withDefaults(defineProps<{ steps: ActivityStep[]; live?: boolean; current?: boolean }>(), {
  live: false,
  current: false,
})
const activeDelta = useBranchView().delta

const thoughts = computed(() => props.steps.filter((s): s is ActivityThought => s.kind === 'thought'))
const views = computed<ToolRowView[]>(() =>
  props.steps.flatMap((s) => (s.kind === 'tools' ? s.rows.map(toolRowView) : [])),
)
const edits = computed(() => views.value.filter(isFileEdit))
const running = computed(() => props.steps.find((s): s is ActivityRunning => s.kind === 'running') ?? null)
const hasCatalog = computed(() => props.steps.some((s) => s.kind === 'catalog'))
const recalls = computed(() => props.steps.filter((s): s is ActivityRecall => s.kind === 'recall'))
const recalledCount = computed(() => recalls.value.reduce((n, s) => n + s.nodes.length, 0))
/** Anything to fold? A lone thought keeps its own disclosure instead. */
const folds = computed(
  () => views.value.length > 0 || running.value !== null || hasCatalog.value || recalls.value.length > 0,
)

/** The live step is still reasoning (its answer / tool calls haven't started). */
const thinking = computed(() =>
  thoughts.value.some((t) => !t.node && activeDelta.value?.reasonMs === undefined),
)

/** The thinking, as a phrase: live, or the kernel's timing summed over the run. */
function thoughtPhrase(): string {
  if (thinking.value) return t('tools.phrase.thinking')
  let ms = 0
  for (const s of thoughts.value) {
    const n = s.node
    ms += n?.body.type === 'assistant' ? (n.body.reason_ms ?? 0) : (activeDelta.value?.reasonMs ?? 0)
  }
  return t('tools.phrase.thoughtFor', { duration: fmtThought(ms) })
}

// The frontier's span ticks while it runs.
const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | undefined
watch(
  () => props.current,
  (c) => {
    if (timer) clearInterval(timer)
    timer = c ? setInterval(() => (now.value = Date.now()), 1000) : undefined
  },
  { immediate: true },
)
onBeforeUnmount(() => timer && clearInterval(timer))

/** The part of a node's time that belongs to this run, as [start, end]. A
 * step that also wrote answer text straddles two runs — its reasoning ends
 * the one before the text, its calls open the one after — so a thought
 * counts only its reasoning, and the step behind a call only from the end
 * of its text (its calls come last). A step without text is all this run's. */
function spanOf(n: Node, as: 'thought' | 'caller'): [number, number] {
  const end = n.at
  const start = end - (n.dur_ms ?? 0)
  const b = n.body
  if (b.type !== 'assistant' || !contentText(b.content).trim()) return [start, end]
  return as === 'thought' ? [start, start + (b.reason_ms ?? 0)] : [end, end]
}

/** Wall time of the run: from the first of its spans' starts to the last
 * end (now, while it is the frontier). */
const spanMs = computed(() => {
  const spans: [number, number][] = []
  for (const s of props.steps) {
    if (s.kind === 'thought' && s.node) spans.push(spanOf(s.node, 'thought'))
    if (s.kind === 'tools') {
      for (const r of s.rows) {
        spans.push(spanOf(r.result, 'caller'))
        if (r.assistant) spans.push(spanOf(r.assistant, 'caller'))
      }
    }
    if (s.kind === 'running') for (const c of s.calls) spans.push(spanOf(c.assistant, 'caller'))
  }
  if (!spans.length) return 0
  const start = Math.min(...spans.map((s) => s[0]))
  const end = props.current ? now.value : Math.max(...spans.map((s) => s[1]))
  return Math.max(0, end - start)
})

/** The run's line: a lone call names itself; more say how long they took
 * and how many there were; without calls, the thinking (or recall /
 * catalog, when that is all there is). */
const summaryParts = computed<PhrasePart[]>(() => {
  const calls = views.value.length + (running.value?.calls.length ?? 0)
  let parts: PhrasePart[]
  if (calls === 1 && views.value.length === 1) parts = toolPhrase(views.value[0]!)
  else if (calls)
    parts = [{ text: t('tools.activity.worked', { duration: fmtThought(spanMs.value), count: calls }) }]
  else if (thoughts.value.length) parts = [{ text: thoughtPhrase() }]
  else if (recalledCount.value) parts = [{ text: t('cards.pills.recalled', { count: recalledCount.value }) }]
  else parts = [{ text: t('tools.phrase.catalog') }]
  const first = parts[0]
  // Only English capitalizes the line; Chinese has no case to fix.
  if (first && locale.value.startsWith('en')) {
    parts = [{ ...first, text: first.text.charAt(0).toUpperCase() + first.text.slice(1) }, ...parts.slice(1)]
  }
  return parts
})
const errorCount = computed(() => views.value.filter((v) => v.status === 'error').length)
const blockedCount = computed(() => views.value.filter((v) => v.status === 'blocked').length)
/** The run's `agent` calls (a plugin's are its script's business). */
const agents = computed(() => views.value.filter((v) => v.name === 'agent'))

/** Each call's full facts, one per line, for the summary's tooltip. */
const summaryTitle = computed(() => views.value.map(toolRowTitle).join('\n'))

/** The opened body, in order: a tools step splits around its edits, which
 *  show as cards between the rows of the calls before and after them. */
type Block =
  | { kind: 'step'; key: string; step: ActivityStep }
  | { kind: 'rows'; key: string; rows: ToolRowInfo[] }
  | { kind: 'edit'; key: string; view: ToolRowView }
const blocks = computed<Block[]>(() => {
  const out: Block[] = []
  for (const s of props.steps) {
    if (s.kind !== 'tools') {
      out.push({ kind: 'step', key: stepKey(s), step: s })
      continue
    }
    let rows: ToolRowInfo[] = []
    const flush = (): void => {
      if (rows.length) out.push({ kind: 'rows', key: `rows:${rows[0]!.result.id}`, rows })
      rows = []
    }
    for (const row of s.rows) {
      const v = toolRowView(row)
      if (isFileEdit(v)) {
        flush()
        out.push({ kind: 'edit', key: `edit:${row.result.id}`, view: v })
      } else rows.push(row)
    }
    flush()
  }
  return out
})

function stepKey(s: ActivityStep): string {
  if (s.kind === 'thought') return s.node?.id ?? 'thought:live'
  if (s.kind === 'catalog') return `catalog:${s.node.id}`
  if (s.kind === 'recall') return `recall:${s.nodes[0]?.id ?? ''}`
  if (s.kind === 'running') return 'running'
  return `tools:${s.rows[0]?.result.id ?? ''}`
}

/** Tri-state: null = auto — open while it is the turn's frontier, folded
 *  once the model moves on. */
const manual = ref<boolean | null>(null)
const open = computed(() => (manual.value !== null ? manual.value : props.current))
/** Blocks present when it opens reveal with it; later ones fade in. */
const revealed = ref<Set<string>>(new Set())
function toggleOpen(): void {
  manual.value = !open.value
  if (manual.value) revealed.value = new Set(blocks.value.map((b) => b.key))
}
</script>

<template>
  <div v-if="!folds" class="flex min-w-0 flex-col gap-2">
    <ThoughtStep v-for="t in thoughts" :key="t.node?.id ?? 'thought:live'" :node="t.node" />
  </div>
  <div v-else class="flex min-w-0 flex-col items-start">
    <div class="flex max-w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <button
        type="button"
        class="reason-toggle max-w-full min-w-0"
        :class="open ? 'text-ink-2' : 'text-ink-3 hover:text-ink-2'"
        :title="summaryTitle || undefined"
        :aria-expanded="open"
        @click="toggleOpen"
      >
        <span v-if="live" class="dot-live size-1.5 flex-none rounded-full bg-brand" />
        <Icon
          v-else-if="!views.length"
          :name="thoughts.length ? 'brain' : 'memory'"
          :size="12"
          class="flex-none"
        />
        <span class="min-w-0 truncate tabular-nums"
          ><span v-for="(p, i) in summaryParts" :key="i" :class="{ 'font-mono font-medium': p.mono }">{{
            p.text
          }}</span></span
        >
        <span v-if="errorCount" class="flex-none text-danger">
          {{ t('tools.activity.error', { count: errorCount }) }}
        </span>
        <span
          v-if="blockedCount"
          v-enter="'enter-stamp'"
          class="flex-none rotate-[-4deg] rounded-xs border-[1.5px] border-danger/60 px-1.5 font-bold tracking-wide text-danger"
          >{{ t('tools.activity.blocked', { count: blockedCount }) }}</span
        >
        <Icon name="chevron" :size="12" class="reason-chevron flex-none" :class="{ 'rotate-90': open }" />
      </button>
    </div>

    <div v-if="open" class="activity-body reveal">
      <template v-for="b in blocks" :key="b.key">
        <EditCard
          v-if="b.kind === 'edit'"
          v-enter="revealed.has(b.key) ? false : 'enter-fade'"
          :view="b.view"
        />
        <ToolList
          v-else-if="b.kind === 'rows'"
          v-enter="revealed.has(b.key) ? false : 'enter-fade'"
          :rows="b.rows"
        />
        <RecallList
          v-else-if="b.step.kind === 'recall'"
          v-enter="revealed.has(b.key) ? false : 'enter-fade'"
          :nodes="b.step.nodes"
        />
        <CatalogList
          v-else-if="b.step.kind === 'catalog'"
          v-enter="revealed.has(b.key) ? false : 'enter-fade'"
          :node="b.step.node"
        />
        <RunningTools v-else-if="b.step.kind === 'running'" v-enter="'enter-fade'" :step="b.step" />
        <!-- without calls the run's line already names the thinking: the
             reasoning shows bare, not behind a second disclosure -->
        <ThoughtStep
          v-else-if="b.step.kind === 'thought'"
          v-enter="revealed.has(b.key) ? false : 'enter-fade'"
          :node="b.step.node"
          :flat="!views.length && !running"
        />
      </template>
    </div>
    <!-- folded, what it changed and what still runs stay in view -->
    <div v-else-if="edits.length || running" class="mt-2 flex w-full flex-col gap-2">
      <EditCard v-for="v in edits" :key="v.row.result.id" :view="v" />
      <RunningTools v-if="running" :step="running" />
    </div>
    <!-- the sub-agents it started stand outside the fold -->
    <div v-if="agents.length" class="mt-2 flex w-full flex-col gap-1.5">
      <AgentCard v-for="v in agents" :key="v.row.result.id" v-enter="'enter-fade'" :view="v" />
    </div>
  </div>
</template>
