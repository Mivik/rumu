<script setup lang="ts">
// A background call's task at the end of its row: its status as an icon,
// then its id; the status word and what it runs are in the tooltip.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openTaskDetail, state } from '../../lib/store'
import { taskLabel, taskState, taskStatusIcon, taskStatusWord } from '../../lib/tasks'
import Chip from '../ui/Chip.vue'

const props = defineProps<{ id: string }>()

const task = computed(() => state.tasks[props.id])
const status = computed(() => (task.value ? taskState(task.value) : 'running'))
const title = computed(() => {
  const head = t('tools.row.task', { id: props.id, status: taskStatusWord(status.value) })
  return task.value ? `${head}\n${taskLabel(task.value)}` : head
})
</script>

<template>
  <Chip
    :icon="taskStatusIcon(status)"
    :tone="status === 'failed' ? 'danger' : 'tool'"
    mono
    clickable
    :title="title"
    @click.stop="openTaskDetail(id)"
  >
    {{ id }}
  </Chip>
</template>
