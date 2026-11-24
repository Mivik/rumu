<script setup lang="ts">
// A background task's output beside the chat: how it is doing, then the
// same terminal body as its opened row (BashView), following the end while
// it streams.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import { vFollow } from '../../lib/motion'
import { callById } from '../../lib/state/derived'
import { ensureAgentNodes, killTask, openTaskDetail, state, taskOutput, useTaskWatch } from '../../lib/store'
import { taskCommand, taskElapsed, taskState, taskStatusIcon, taskStatusWord } from '../../lib/tasks'
import { argsOf, bashExtraArgs } from '../../lib/toolrows'
import { toolIcon } from '../../lib/toolIcon'
import Icon from '../ui/Icon.vue'
import BashView from './BashView.vue'
import PanelBack from './PanelBack.vue'

const task = computed(() => (state.taskDetail ? (state.tasks[state.taskDetail] ?? null) : null))
const running = computed(() => task.value?.status === 'running')

const command = computed(() => (task.value ? taskCommand(task.value) : ''))
const args = computed(() =>
  command.value && task.value ? bashExtraArgs(argsOf(callById(task.value.call_id))) : [],
)

const box = ref<HTMLElement | null>(null)

/** Scrolled before the re-render, so `v-follow` stops following and the
 * command just unfolded stays in view. */
function onCommandToggle(open: boolean): void {
  if (open && box.value) box.value.scrollTop = 0
}

// A sub-agent's nodes (and so the call) load only on demand.
watch(
  task,
  (tk) => {
    if (tk && !taskCommand(tk)) void ensureAgentNodes(tk.branch)
  },
  { immediate: true },
)

const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  timer = setInterval(() => (now.value = Date.now()), 1000)
})
onBeforeUnmount(() => clearInterval(timer))

useTaskWatch(() => (task.value && running.value ? [task.value.id] : []))

/** The live output while it runs, then the final result. */
const output = computed(() => {
  const tk = task.value
  return tk ? (running.value ? taskOutput(tk.id) : (tk.result?.text ?? '')) : ''
})
</script>

<template>
  <section v-if="task" class="sheet flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface">
    <header class="flex min-h-14 flex-none items-center gap-2.5 border-b border-line-soft px-4">
      <PanelBack />
      <Icon
        :name="task.kind.type === 'agent' ? 'agent' : toolIcon(task.kind.name)"
        :size="16"
        class="flex-none"
        :class="task.kind.type === 'agent' ? 'text-agent' : 'text-tool'"
      />
      <span class="flex min-w-0 flex-col">
        <span
          v-if="task.kind.type === 'tool'"
          class="truncate font-mono text-meta"
          :title="task.kind.args_summary"
        >
          <span class="font-semibold text-tool">{{ task.kind.name }}</span>
          <span class="ml-1.5 text-ink">{{ task.kind.args_summary }}</span>
        </span>
        <span v-else class="truncate text-meta font-semibold" :title="task.summary">{{ task.summary }}</span>
        <span class="flex items-center gap-3 text-micro text-ink-3">
          <span class="flex items-center gap-1.5" :class="{ 'text-danger': task.is_error }">
            <span v-if="running" class="dot-live dot-run size-1.5 rounded-full bg-pine" />
            <Icon v-else :name="taskStatusIcon(taskState(task))" :size="12" />
            {{ taskStatusWord(taskState(task)) }}
          </span>
          <span class="flex items-center gap-1">
            <Icon name="timer" :size="12" />
            <span class="font-mono tabular-nums">{{ taskElapsed(task, now) }}</span>
          </span>
          <span class="font-mono">{{ task.id }}</span>
        </span>
      </span>
      <span class="ml-auto flex flex-none items-center gap-0.5">
        <button
          v-if="running"
          type="button"
          class="icon-btn size-7 hover:text-danger"
          :title="t('tools.tasks.kill', { id: task.id })"
          :aria-label="t('tools.tasks.killAria', { id: task.id })"
          @click="killTask(task.id)"
        >
          <Icon name="stop" :size="14" />
        </button>
        <button
          type="button"
          class="icon-btn size-7"
          :title="t('tools.tasks.close')"
          :aria-label="t('tools.tasks.close')"
          @click="openTaskDetail(task.id)"
        >
          <Icon name="close" :size="15" />
        </button>
      </span>
    </header>

    <div :key="task.id" ref="box" v-follow="running" class="min-h-0 flex-1 overflow-auto">
      <BashView
        :output="output"
        :failed="task.is_error"
        :running="running"
        :command="command"
        :args="args"
        fill
        @toggle="onCommandToggle"
      />
    </div>

    <footer
      v-if="task.truncated"
      class="flex flex-none items-center border-t border-line-soft px-5 py-2 text-micro text-ink-3"
    >
      {{ t('common.truncated') }}
    </footer>
  </section>
</template>
