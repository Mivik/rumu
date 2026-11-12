<script setup lang="ts">
// A sub-agent the turn started, outside the folded activity run: its task
// and how it is doing. Click → its transcript beside the chat.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openAgentView, state } from '../../lib/store'
import type { ToolRowView } from '../../lib/toolrows'
import { argsOf } from '../../lib/toolrows'
import { taskStatusWord } from '../../lib/tasks'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ view: ToolRowView }>()

const task = computed(() => {
  const raw = argsOf(props.view.row.call)?.task
  return typeof raw === 'string' ? raw.trim() : props.view.callFull
})
const child = computed(() => props.view.agentChild)
const open = computed(() => !!child.value && state.agentView === child.value.id)

/** Running while its task or its branch is; otherwise the task's outcome. */
const running = computed(
  () => props.view.taskStatus === 'running' || (!!child.value && !!state.busy[child.value.id]),
)
const failed = computed(
  () =>
    props.view.status !== 'ok' || props.view.taskStatus === 'failed' || props.view.taskStatus === 'cancelled',
)
const statusText = computed(() => {
  if (running.value) return taskStatusWord('running')
  if (props.view.taskStatus && props.view.taskStatus !== 'done') return taskStatusWord(props.view.taskStatus)
  return props.view.status === 'ok' ? '' : props.view.statusLabel
})
</script>

<template>
  <button
    type="button"
    :data-path-node="view.row.result.id"
    class="agent-card"
    :class="{ 'is-open': open }"
    :disabled="!child"
    :title="task"
    @click="child && openAgentView(open ? null : child.id)"
  >
    <span class="agent-card-icon">
      <Icon name="agent" :size="16" />
    </span>
    <span class="flex min-w-0 flex-1 flex-col items-start">
      <span class="font-semibold text-ink">{{ t('chat.agentView.title') }}</span>
      <span class="max-w-full truncate text-micro text-ink-3">{{ task }}</span>
    </span>
    <span
      v-if="statusText"
      class="flex flex-none items-center gap-1.5 text-micro"
      :class="failed && !running ? 'text-danger' : 'text-ink-3'"
    >
      <span v-if="running" class="dot-live dot-task size-1.5 rounded-full bg-pine" />
      {{ statusText }}
    </span>
  </button>
</template>

<style scoped>
.agent-card {
  display: flex;
  width: min(100%, 480px);
  align-items: center;
  gap: 10px;
  padding: 8px 12px 8px 8px;
  border: 1px solid var(--line);
  border-radius: var(--radius-control);
  background: var(--surface);
  box-shadow: var(--shadow-soft-v);
  font-size: var(--text-meta);
  text-align: left;
  transition:
    border-color 0.15s,
    box-shadow 0.15s;
}
.agent-card:not(:disabled):hover {
  border-color: color-mix(in srgb, var(--agent) 45%, var(--line));
}
.agent-card.is-open {
  border-color: var(--agent);
  box-shadow: 0 0 0 1px var(--agent);
}
.agent-card-icon {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: calc(var(--radius-control) - 2px);
  background: var(--agent-soft);
  color: var(--agent);
  line-height: 0;
}
</style>
