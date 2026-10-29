<script setup lang="ts">
// A small strip of the active branch's running background tasks, plus the
// detail of a task opened from a completion pill / a tool row row: status,
// elapsed time, a kill button, and the live buffered output.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import { branchTasks, killTask, openTaskDetail, runningTasks, state } from '../../lib/store'
import { taskElapsed, taskLabel, taskStatusWord } from '../../lib/tasks'
import Icon from '../ui/Icon.vue'

/** Recompute elapsed times once a second. */
const now = ref(Date.now())
let timer: ReturnType<typeof setInterval> | undefined
onMounted(() => {
  timer = setInterval(() => (now.value = Date.now()), 1000)
})
onBeforeUnmount(() => clearInterval(timer))

const detail = computed(() => (state.taskDetail ? state.tasks[state.taskDetail] : null))

/** Live text when the page saw it arrive; otherwise the final result. */
const detailOutput = computed(() => {
  const t = detail.value
  if (!t) return ''
  const live = state.taskOutput[t.id] ?? ''
  return live || t.result?.text || ''
})

function elapsed(t: (typeof branchTasks.value)[number]): string {
  return taskElapsed(t, now.value)
}
</script>

<template>
  <div
    v-if="runningTasks.length || detail"
    class="mx-auto mb-1.5 flex w-full max-w-[var(--col-max)] flex-col gap-1.5"
  >
    <div v-if="runningTasks.length" class="flex flex-wrap items-center gap-1.5">
      <span class="flex items-center gap-1.5 text-micro font-semibold text-ink-3">
        <span class="dot-live size-1.5 rounded-full bg-brand" />
        {{ t('tools.tasks.label') }}
      </span>
      <span v-for="tk in runningTasks" :key="tk.id" class="chip tone-tool text-micro" :title="taskLabel(tk)">
        <button type="button" class="flex min-w-0 items-center gap-1.5" @click="openTaskDetail(tk.id)">
          <Icon name="tool" :size="11" />
          <span class="font-mono font-semibold">{{ tk.id }}</span>
          <span class="max-w-[220px] truncate font-mono text-ink-2">{{ taskLabel(tk) }}</span>
          <span class="font-mono text-ink-3">{{ elapsed(tk) }}</span>
        </button>
        <button
          type="button"
          class="icon-btn size-4 text-ink-3 hover:text-danger"
          :title="t('tools.tasks.kill', { id: tk.id })"
          :aria-label="t('tools.tasks.killAria', { id: tk.id })"
          @click="killTask(tk.id)"
        >
          <Icon name="close" :size="10" />
        </button>
      </span>
    </div>

    <div v-if="detail" class="rounded-control border border-line bg-surface-2/40 px-3 py-2">
      <div class="flex min-w-0 items-center gap-2">
        <Icon name="tool" :size="12" class="flex-none text-tool" />
        <span class="font-mono text-meta font-semibold">{{ detail.id }}</span>
        <span class="text-micro text-ink-3">{{ taskStatusWord(detail.status) }}</span>
        <span class="min-w-0 truncate font-mono text-meta text-ink-2">{{ taskLabel(detail) }}</span>
        <span class="ml-auto flex flex-none items-center gap-1.5 text-micro text-ink-3">
          <span v-if="detail.truncated">{{ t('common.truncated') }}</span>
          <span class="font-mono">{{ elapsed(detail) }}</span>
          <button
            v-if="detail.status === 'running'"
            type="button"
            class="icon-btn size-5 text-ink-3 hover:text-danger"
            :title="t('tools.tasks.kill', { id: detail.id })"
            @click="killTask(detail.id)"
          >
            <Icon name="close" :size="11" />
          </button>
          <button
            type="button"
            class="icon-btn size-5 text-ink-3"
            :title="t('tools.tasks.close')"
            @click="state.taskDetail = null"
          >
            <Icon name="close" :size="11" />
          </button>
        </span>
      </div>
      <pre class="code mt-1.5 max-h-56 overflow-auto">{{ detailOutput || t('common.noOutput') }}</pre>
    </div>
  </div>
</template>
