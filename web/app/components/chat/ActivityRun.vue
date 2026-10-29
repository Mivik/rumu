<script setup lang="ts">
// The work between two pieces of an assistant turn's visible text — each
// step's reasoning and its tool calls. Once a call has run, it folds by
// default into one quiet summary line of what the calls did:
//   Read 3 files, ran 2 commands · 1 error [blocked] ›
// (a lone call names its target: "Read store.ts"). The line doesn't count the
// thoughts; while the model thinks, it leads with the live dot. Exploration
// pills stay on the folded line so the jump to the child branch is one click.
// Opened, the steps show in order under a hairline: each thought as its own
// disclosure, each run of calls as a ToolList box. A run without calls
// (reasoning right before the answer) has nothing to summarize: its thought
// stands bare.
import { computed, ref } from 'vue'
import { t, locale } from '../../i18n'
import type { ActivityStep, ActivityThought, ActivityTools } from '../../lib/store'
import { activeDelta, selectBranch } from '../../lib/store'
import type { PhrasePart, ToolRowView } from '../../lib/toolrows'
import { toolPhrases, toolRowTitle, toolRowView } from '../../lib/toolrows'
import { shortId } from '../../lib/util'
import { vEnter } from '../../lib/motion'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import ThoughtStep from './ThoughtStep.vue'
import ToolList from './ToolList.vue'

const props = defineProps<{ steps: ActivityStep[] }>()

const thoughts = computed(() => props.steps.filter((s): s is ActivityThought => s.kind === 'thought'))
const views = computed<ToolRowView[]>(() =>
  props.steps.flatMap((s) => (s.kind === 'tools' ? (s as ActivityTools).rows.map(toolRowView) : [])),
)

/** The live step is still reasoning (its answer / tool calls haven't started). */
const thinking = computed(() =>
  thoughts.value.some((t) => !t.node && activeDelta.value?.reasonMs === undefined),
)

/** The calls' phrases joined by commas, the first one capitalized. */
const summary = computed<PhrasePart[]>(() => {
  const parts = toolPhrases(views.value).flatMap((p, i) =>
    i ? [{ text: t('tools.phrase.separator') }, ...p] : p,
  )
  const first = parts[0]
  // Only English capitalizes the line; Chinese has no case to fix.
  const capitalize = locale.value.startsWith('en')
  if (first && capitalize) {
    parts[0] = { ...first, text: first.text.charAt(0).toUpperCase() + first.text.slice(1) }
  }
  return parts
})

const errorCount = computed(() => views.value.filter((v) => v.status === 'error').length)
const blockedCount = computed(() => views.value.filter((v) => v.status === 'blocked').length)
const agents = computed(() => views.value.filter((v) => v.agentChild))

/** Each call's full facts, one per line, for the summary's tooltip. */
const summaryTitle = computed(() => views.value.map(toolRowTitle).join('\n'))

function stepKey(s: ActivityStep): string {
  if (s.kind === 'thought') return s.node?.id ?? 'thought:live'
  return `tools:${s.rows[0]?.result.id ?? ''}`
}

/** Folded by default; steps present when it opens reveal with it, later ones fade in. */
const open = ref(false)
const revealed = ref<Set<string>>(new Set())
function toggleOpen(): void {
  open.value = !open.value
  if (open.value) revealed.value = new Set(props.steps.map(stepKey))
}
</script>

<template>
  <div v-if="!views.length" class="flex min-w-0 flex-col gap-2">
    <ThoughtStep v-for="t in thoughts" :key="t.node?.id ?? 'thought:live'" :node="t.node" />
  </div>
  <div v-else class="flex min-w-0 flex-col items-start">
    <div class="flex max-w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <button
        type="button"
        class="reason-toggle max-w-full min-w-0"
        :class="open ? 'text-ink-2' : 'text-ink-3 hover:text-ink-2'"
        :title="summaryTitle"
        :aria-expanded="open"
        @click="toggleOpen"
      >
        <span v-if="thinking" class="dot-live size-1.5 flex-none rounded-full bg-brand" />
        <span class="min-w-0 truncate"
          ><span v-for="(p, i) in summary" :key="i" :class="{ 'font-mono font-medium': p.mono }">{{
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
        <ThoughtStep
          v-if="s.kind === 'thought'"
          v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'"
          :node="s.node"
        />
        <ToolList v-else v-enter="revealed.has(stepKey(s)) ? false : 'enter-fade'" :rows="s.rows" />
      </template>
    </div>
  </div>
</template>
