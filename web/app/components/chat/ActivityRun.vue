<script setup lang="ts">
// The work between two pieces of an assistant turn's visible text — the cards
// recalled for it, the card catalog handed to the model, each step's reasoning
// and its tool calls. It folds into one quiet summary line of what happened:
//   Read 3 files · 1 error [blocked] ›
// (a lone call names its target: "Read store.ts"). Recall and catalog go with
// every turn, so neither shows on the line; the opened body still renders
// them. With tool calls the line doesn't count the thoughts; without any
// (reasoning right before the answer) it names the thinking instead: "Thought
// for 2s". While the model thinks, it leads with the live dot, and a run with
// no calls opens by itself so the reasoning streams in view, folding once the
// answer starts unless toggled by hand. Exploration pills stay on the folded
// line so the jump to the child branch is one click.
// Opened, the steps show in order under a hairline: the catalog's cards,
// each thought, each run of calls as a ToolList box. A bare thought — no
// catalog, no calls — keeps its own disclosure (ThoughtStep).
import { computed, ref } from 'vue'
import { t, locale } from '../../i18n'
import type { ActivityRecall, ActivityStep, ActivityThought, ActivityTools } from '../../lib/store'
import { activeDelta, selectBranch } from '../../lib/store'
import type { PhrasePart, ToolRowView } from '../../lib/toolrows'
import { toolPhrases, toolRowTitle, toolRowView } from '../../lib/toolrows'
import { fmtThought, shortId } from '../../lib/util'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import CatalogList from './CatalogList.vue'
import RecallList from './RecallList.vue'
import ThoughtStep from './ThoughtStep.vue'
import ToolList from './ToolList.vue'

const props = defineProps<{ steps: ActivityStep[] }>()

const thoughts = computed(() => props.steps.filter((s): s is ActivityThought => s.kind === 'thought'))
const views = computed<ToolRowView[]>(() =>
  props.steps.flatMap((s) => (s.kind === 'tools' ? (s as ActivityTools).rows.map(toolRowView) : [])),
)
const hasCatalog = computed(() => props.steps.some((s) => s.kind === 'catalog'))
const recalls = computed(() => props.steps.filter((s): s is ActivityRecall => s.kind === 'recall'))
const recalledCount = computed(() => recalls.value.reduce((n, s) => n + s.nodes.length, 0))
/** Anything to fold? A lone thought keeps its own disclosure instead. */
const folds = computed(() => views.value.length > 0 || hasCatalog.value || recalls.value.length > 0)

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

/** The phrases joined by commas, the first one capitalized. Recall and the
 *  catalog are not phrases: they go with every turn, so they stay off the
 *  line and live in the opened body. */
const summary = computed<PhrasePart[][]>(() => {
  const phrases: PhrasePart[][] = []
  phrases.push(...toolPhrases(views.value))
  if (!views.value.length && thoughts.value.length) phrases.push([{ text: thoughtPhrase() }])
  return phrases
})
const summaryParts = computed<PhrasePart[]>(() => {
  const parts = summary.value.flatMap((p, i) => (i ? [{ text: t('tools.phrase.separator') }, ...p] : p))
  const first = parts[0]
  // Only English capitalizes the line; Chinese has no case to fix.
  if (first && locale.value.startsWith('en')) {
    parts[0] = { ...first, text: first.text.charAt(0).toUpperCase() + first.text.slice(1) }
  }
  return parts
})
/** A line with no phrase (a run that only recalled or read the catalog) still
 *  needs an accessible name. */
const summaryLabel = computed(() => {
  if (summaryParts.value.length) return undefined
  if (hasCatalog.value) return t('tools.phrase.catalog')
  if (recalledCount.value) return t('cards.pills.recalled', { count: recalledCount.value })
  return undefined
})

const errorCount = computed(() => views.value.filter((v) => v.status === 'error').length)
const blockedCount = computed(() => views.value.filter((v) => v.status === 'blocked').length)
const agents = computed(() => views.value.filter((v) => v.agentChild))

/** Each call's full facts, one per line, for the summary's tooltip. */
const summaryTitle = computed(() => views.value.map(toolRowTitle).join('\n'))

function stepKey(s: ActivityStep): string {
  if (s.kind === 'thought') return s.node?.id ?? 'thought:live'
  if (s.kind === 'catalog') return `catalog:${s.node.id}`
  if (s.kind === 'recall') return `recall:${s.nodes[0]?.id ?? ''}`
  return `tools:${s.rows[0]?.result.id ?? ''}`
}

/** Tri-state: null = auto — a run without calls stays open while its model
 *  thinks, so the reasoning streams in view; everything else starts folded. */
const manual = ref<boolean | null>(null)
const open = computed(() => (manual.value !== null ? manual.value : thinking.value && !views.value.length))
/** Steps present when it opens reveal with it; later ones fade in. */
const revealed = ref<Set<string>>(new Set())
function toggleOpen(): void {
  manual.value = !open.value
  if (manual.value) revealed.value = new Set(props.steps.map(stepKey))
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
        :aria-label="summaryLabel"
        :aria-expanded="open"
        @click="toggleOpen"
      >
        <span v-if="thinking" class="dot-live size-1.5 flex-none rounded-full bg-brand" />
        <Icon v-else-if="!views.length" name="brain" :size="12" class="flex-none" />
        <span class="min-w-0 truncate"
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
        <Icon name="chevron" :size="10" class="reason-chevron flex-none" :class="{ 'rotate-90': open }" />
      </button>
      <template v-if="!open">
        <Chip
          v-for="v in agents"
          :key="v.row.result.id"
          icon="agent"
          icon-after="external"
          tone="agent"
          clickable
          :title="t('tools.row.subAgentTitle', { id: shortId(v.agentChild!.id) })"
          @click="selectBranch(v.agentChild!.id)"
        >
          {{ t('tools.row.openSubAgent') }}
        </Chip>
      </template>
    </div>

    <div v-if="open" class="activity-body reveal">
      <template v-for="s in steps" :key="stepKey(s)">
        <RecallList
          v-if="s.kind === 'recall'"
          v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'"
          :nodes="s.nodes"
        />
        <CatalogList
          v-else-if="s.kind === 'catalog'"
          v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'"
          :node="s.node"
        />
        <!-- without calls the run's line already names the thinking: the
             reasoning shows bare, not behind a second disclosure -->
        <ThoughtStep
          v-else-if="s.kind === 'thought'"
          v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'"
          :node="s.node"
          :flat="!views.length"
        />
        <ToolList v-else v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'" :rows="s.rows" />
      </template>
    </div>
  </div>
</template>
