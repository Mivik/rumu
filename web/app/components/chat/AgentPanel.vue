<script setup lang="ts">
// A sub-agent beside the chat (over it below xl), without leaving the main
// line: the same conversation and composer as the chat's, on the sub-agent's
// branch. Its turns offer no edit, regenerate or fork (the branch belongs to
// the call that started it); the composer sends it a message, which runs it
// again if it has finished. It opens on the task, shown as the prompt it is.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openAgentView, provideBranchView, state, subView } from '../../lib/store'
import Icon from '../ui/Icon.vue'
import BranchChat from './BranchChat.vue'

const view = subView()
provideBranchView(view)

const branch = computed(() => (state.agentView ? (state.branches[state.agentView] ?? null) : null))
const kind = computed(() => (branch.value?.kind.type === 'agent' ? branch.value.kind : null))

/** The sub-agent that started this one, when it is nested. */
const parent = computed(() => {
  const p = kind.value ? state.branches[kind.value.parent_branch] : undefined
  return p?.kind.type === 'agent' ? p : null
})
</script>

<template>
  <section
    v-if="branch && kind"
    data-agent-panel
    class="sheet flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface"
  >
    <header class="flex min-h-14 flex-none items-center gap-2 border-b border-line-soft px-4">
      <button
        v-if="parent"
        class="icon-btn size-7 flex-none"
        :title="t('chat.agentView.back')"
        :aria-label="t('chat.agentView.back')"
        @click="openAgentView(parent.id)"
      >
        <Icon name="back" :size="14" />
      </button>
      <Icon name="agent" :size="16" class="flex-none text-agent" />
      <span class="flex flex-none items-center gap-1.5 text-[15px] font-bold">
        {{ t('chat.agentView.title') }}
        <span v-if="view.busy.value" class="dot-live dot-task size-1.5 rounded-full bg-pine" />
      </span>
      <span class="min-w-0 flex-1 truncate text-meta text-ink-3" :title="kind.task">{{ kind.task }}</span>
      <button
        class="icon-btn size-7 flex-none"
        :title="t('chat.agentView.close')"
        :aria-label="t('chat.agentView.close')"
        @click="openAgentView(null)"
      >
        <Icon name="close" :size="15" />
      </button>
    </header>
    <BranchChat />
  </section>
</template>
