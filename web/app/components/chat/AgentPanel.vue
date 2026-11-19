<script setup lang="ts">
// A sub-agent's transcript, beside the chat (over it below xl), without
// switching the session to its branch. It streams while the sub-agent runs
// and follows the end unless scrolled away. Read-only: a sub-agent's branch
// belongs to the call that started it, so its turns offer no edit,
// regenerate or fork. It opens on the task, shown as the prompt it is.
import { computed, nextTick, ref, watch, watchEffect } from 'vue'
import { t } from '../../i18n'
import {
  buildTimeline,
  callsOf,
  ensureAgentNodes,
  openAgentView,
  pathOf,
  provideBranchView,
  state,
} from '../../lib/store'
import type { Node } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import AssistantGroup from './AssistantGroup.vue'
import ContextPills from './ContextPills.vue'
import CompactionRow from './CompactionRow.vue'
import UserRow from './UserRow.vue'

const branch = computed(() => (state.agentView ? (state.branches[state.agentView] ?? null) : null))
const kind = computed(() => (branch.value?.kind.type === 'agent' ? branch.value.kind : null))

// The tree leaves a sub-agent's nodes out until it is opened.
watchEffect(() => {
  if (state.agentView) void ensureAgentNodes(state.agentView)
})
const loaded = computed(() => !!state.agentView && state.agentNodes[state.agentView] === 'loaded')

/** The branch's own nodes from its goal prompt on, the prompt showing the
 *  task as it was given (the kernel's sub-agent framing is the model's, not
 *  the reader's). What was seeded before it is the parent's. */
const nodes = computed<Node[]>(() => {
  const b = branch.value
  if (!b || !kind.value || !loaded.value) return []
  const own = pathOf(b, b.forked_from)
  const at = own.findIndex((n) => n.body.type === 'user')
  const prompt = own[at]
  if (!prompt || prompt.body.type !== 'user') return own
  const task: Node = {
    ...prompt,
    body: { ...prompt.body, content: { ...prompt.body.content, text: kind.value.task } },
  }
  return [task, ...own.slice(at + 1)]
})

const id = computed(() => branch.value?.id ?? null)
const busy = computed(() => !!id.value && !!state.busy[id.value])
provideBranchView({
  branch: id,
  nodes,
  delta: computed(() => (id.value ? (state.deltas[id.value] ?? null) : null)),
  busy,
  sub: true,
})

const timeline = computed(() => buildTimeline(nodes.value, callsOf(nodes.value), id.value))
const lastGroup = computed(() => timeline.value.findLastIndex((e) => e.kind === 'group'))

/** The sub-agent that started this one, when it is nested. */
const parent = computed(() => {
  const p = kind.value ? state.branches[kind.value.parent_branch] : undefined
  return p?.kind.type === 'agent' ? p : null
})

// Follow the end while it grows, unless scrolled away from it.
const scrollEl = ref<HTMLElement | null>(null)
const following = ref(true)
function onScroll(): void {
  const el = scrollEl.value
  if (el) following.value = el.scrollHeight - el.scrollTop - el.clientHeight < 90
}
watch(id, () => (following.value = true))
watch(
  [timeline, () => (id.value ? state.deltas[id.value] : null)],
  () => {
    if (!following.value) return
    void nextTick(() => {
      const el = scrollEl.value
      if (el) el.scrollTop = el.scrollHeight
    })
  },
  { deep: true, immediate: true },
)
</script>

<template>
  <section
    v-if="branch && kind"
    class="sheet flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface"
  >
    <header class="flex min-h-14 flex-none items-center gap-2 border-b border-line-soft px-4">
      <button
        v-if="parent"
        class="icon-btn size-7"
        :title="t('chat.agentView.parent')"
        :aria-label="t('chat.agentView.parent')"
        @click="openAgentView(parent.id)"
      >
        <Icon name="back" :size="14" />
      </button>
      <Icon name="agent" :size="16" class="flex-none text-agent" />
      <span class="flex items-center gap-1.5 text-[15px] font-bold">
        {{ t('chat.agentView.title') }}
        <span v-if="busy" class="dot-live dot-task size-1.5 rounded-full bg-pine" />
      </span>
      <button
        class="icon-btn ml-auto size-7 flex-none"
        :title="t('chat.agentView.close')"
        :aria-label="t('chat.agentView.close')"
        @click="openAgentView(null)"
      >
        <Icon name="close" :size="15" />
      </button>
    </header>

    <div ref="scrollEl" class="min-h-0 flex-1 overflow-y-auto px-5 py-4" @scroll="onScroll">
      <div :key="branch.id" class="view-in flex flex-col gap-5">
        <div v-for="(entry, i) in timeline" :key="i" class="min-w-0">
          <UserRow v-if="entry.kind === 'user'" :node="entry.node" :context="entry.contextNodes" />
          <AssistantGroup v-else-if="entry.kind === 'group'" :group="entry" :live="busy && i === lastGroup" />
          <ContextPills v-else-if="entry.kind === 'context'" :nodes="entry.nodes" />
          <CompactionRow v-else-if="entry.kind === 'compaction'" :node="entry.node" />
        </div>
        <p v-if="loaded && !timeline.length" class="text-meta text-ink-3">{{ t('chat.agentView.empty') }}</p>
      </div>
    </div>
  </section>
</template>
