<script lang="ts">
// One side note (a context notice other than recalled cards; see
// ContextPills), as a sentence that reads on its own: the default workspace
// set, a CLAUDE.md loaded, a card that changed, a finished background task,
// a plain note in full, files the user reverted.
import type { TaskStatus } from '../../lib/types'

export type Notice =
  | { kind: 'workspace' | 'agents'; key: string; name: string }
  | { kind: 'update'; key: string; id: string; label: string; text: string }
  | { kind: 'task'; key: string; id: string; status: TaskStatus; text: string }
  | { kind: 'note'; key: string; text: string }
  | { kind: 'revert'; key: string; files: string[]; text: string }

const ICON: Record<Notice['kind'], string> = {
  workspace: 'mount',
  agents: 'file',
  update: 'note',
  task: 'tool',
  note: 'note',
  revert: 'undo',
}
</script>

<script setup lang="ts">
import { t } from '../../i18n'
import { openDrawer, openTaskDetail } from '../../lib/store'
import { taskStatusWord } from '../../lib/tasks'
import { workspaceLabel } from '../../lib/util'
import Icon from '../ui/Icon.vue'

defineProps<{ notice: Notice }>()
</script>

<template>
  <div class="flex max-w-full min-w-0 items-baseline gap-1.5 text-meta text-ink-3">
    <Icon :name="ICON[notice.kind]" :size="12" class="flex-none text-ink-3" />
    <span v-if="notice.kind === 'workspace'" class="min-w-0 truncate">
      {{ t('cards.notes.workspace', { name: workspaceLabel(notice.name) }) }}
    </span>
    <span v-else-if="notice.kind === 'agents'" class="min-w-0 truncate">
      {{ t('cards.notes.loaded', { name: notice.name }) }}
    </span>
    <span v-else-if="notice.kind === 'update'" class="min-w-0" :title="notice.text">
      {{ t('cards.notes.cardBefore') }}
      <button class="card-ref" type="button" @click="openDrawer(notice.id)">{{ notice.label }}</button>
      {{ t('cards.notes.cardAfter') }}
    </span>
    <button
      v-else-if="notice.kind === 'task'"
      type="button"
      class="min-w-0 truncate text-left hover:text-ink"
      :title="notice.text"
      @click="openTaskDetail(notice.id)"
    >
      {{ t('cards.notes.task', { id: notice.id, status: taskStatusWord(notice.status) }) }}
    </button>
    <span v-else-if="notice.kind === 'revert'" class="min-w-0 truncate" :title="notice.text">
      {{
        notice.files.length
          ? t('cards.notes.revert', { count: notice.files.length })
          : t('cards.notes.revertNone')
      }}
    </span>
    <span v-else-if="notice.kind === 'note'" class="min-w-0 whitespace-pre-wrap">{{ notice.text }}</span>
  </div>
</template>
