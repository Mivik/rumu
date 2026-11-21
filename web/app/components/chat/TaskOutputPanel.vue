<script setup lang="ts">
// A background task's output beside the chat: what it runs, how it is doing,
// and its buffered output, following the end while it streams.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import { vFollow } from '../../lib/motion'
import { ensureAgentNodes, killTask, openTaskDetail, state } from '../../lib/store'
import { taskCommand, taskElapsed, taskStatusWord } from '../../lib/tasks'
import { bashExit } from '../../lib/toolrows'
import { toolIcon } from '../../lib/toolIcon'
import Icon from '../ui/Icon.vue'
import PanelBack from './PanelBack.vue'
import TermCommand from './TermCommand.vue'

const task = computed(() => (state.taskDetail ? (state.tasks[state.taskDetail] ?? null) : null))
const running = computed(() => task.value?.status === 'running')

/** The command itself: the task's own label carries only a summary of it
 * (`description` first, cut to 80 chars), and the kernel keeps no args. */
const command = computed(() => (task.value?.kind.type === 'tool' ? taskCommand(task.value) : ''))

// A task started inside a sub-agent branch: its call sits in nodes the tree
// leaves out until that branch's nodes are asked for.
watch(
  task,
  (tk) => {
    if (tk?.kind.type === 'tool' && !taskCommand(tk)) void ensureAgentNodes(tk.branch)
  },
  { immediate: true },
)

const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  timer = setInterval(() => (now.value = Date.now()), 1000)
})
onBeforeUnmount(() => clearInterval(timer))

/** Live text when the page saw it arrive; otherwise the final result. */
const output = computed(() => {
  const tk = task.value
  const { code, body } = bashExit(tk ? state.taskOutput[tk.id] || tk.result?.text || '' : '')
  return { code, body: body.replace(/\n+$/, '') }
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
        <span class="flex items-center gap-1.5 text-micro text-ink-3">
          <span v-if="running" class="dot-live dot-task size-1.5 rounded-full bg-pine" />
          <span :class="{ 'text-danger': task.is_error }">{{
            taskStatusWord(task.status === 'done' && task.is_error ? 'failed' : task.status)
          }}</span>
          <span>·</span>
          <span class="font-mono tabular-nums">{{ taskElapsed(task, now) }}</span>
          <span>·</span>
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

    <!-- The command above its output: the header has room for the summary
         only, and a summary is not something to run again. -->
    <div
      v-if="command"
      class="flex-none border-b border-line-soft bg-surface-2 px-5 py-2 font-mono text-meta leading-[1.55]"
    >
      <TermCommand :command="command" :lines="3" />
    </div>

    <pre
      v-if="output.body"
      :key="task.id"
      v-follow="running"
      class="view-in min-h-0 flex-1 overflow-auto px-5 py-4 font-mono text-micro leading-[1.6] break-words whitespace-pre-wrap"
      >{{ output.body }}</pre>
    <p v-else class="flex-1 px-5 py-4 text-meta text-ink-3">{{ t('common.noOutput') }}</p>

    <footer
      v-if="output.code || task.truncated"
      class="flex flex-none items-center gap-3 border-t border-line-soft px-5 py-2 text-micro"
    >
      <span v-if="output.code" class="font-mono font-semibold text-danger">
        {{ t('tools.result.exit', { code: output.code }) }}
      </span>
      <span v-if="task.truncated" class="text-ink-3">{{ t('common.truncated') }}</span>
    </footer>
  </section>
</template>
