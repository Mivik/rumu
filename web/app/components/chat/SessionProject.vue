<script setup lang="ts">
// The open session's project, beside its view tabs: its chip (seal and
// name; it opens the project's page) when it is in one, and a menu that
// moves it to another project or out of its own.
import { computed } from 'vue'
import { t } from '../../i18n'
import { moveSession, state } from '../../lib/store'
import { openPage } from '../../lib/router'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import ProjectChoices from './ProjectChoices.vue'

/** The session as the list has it: a move shows before its push. */
const info = computed(() => state.sessions.find((s) => s.id === state.sessionId) ?? state.session)
const id = computed(() => info.value?.project ?? null)
const project = computed(() => state.projects.find((p) => p.id === id.value) ?? null)

function pick(to: string | null, close: () => void): void {
  close()
  if (info.value && to !== id.value) void moveSession(info.value.id, to)
}
</script>

<template>
  <span class="ml-auto flex min-w-0 items-center gap-1 pb-1.5">
    <button
      v-if="id"
      type="button"
      class="scope-tag"
      :title="t('chat.sidebar.projectSettings')"
      @click="openPage('projects', id)"
    >
      <ProjectSeal v-if="project" :project="project" :size="14" />
      <Icon v-else name="mount" :size="13" class="flex-none text-tool" />
      <span class="truncate">{{ project?.name ?? id.slice(0, 8) }}</span>
    </button>
    <Popover :label="t('chat.header.project')" placement="bottom-end">
      <template #trigger="{ open, toggle }">
        <button
          type="button"
          class="icon-btn size-6 flex-none"
          :class="{ 'bg-surface-2 text-ink': open }"
          :title="t('chat.header.moveProject')"
          :aria-label="t('chat.header.moveProject')"
          :aria-expanded="open"
          @click="toggle"
        >
          <Icon :name="id ? 'edit' : 'plus'" :size="13" />
        </button>
      </template>
      <template #default="{ close }">
        <div class="panel-head">{{ t('chat.header.project') }}</div>
        <ProjectChoices :selected="id" @pick="pick($event, close)" />
      </template>
    </Popover>
  </span>
</template>
