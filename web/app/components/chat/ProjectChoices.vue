<script setup lang="ts">
// A one-of list of the projects, "No project" first, the `selected` one
// ticked: the landing pick of a new session's project (SessionPickers) and
// the open session's move (SessionProject) both choose from it.
import { t } from '../../i18n'
import { state } from '../../lib/store'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'

defineProps<{ selected: string | null }>()
const emit = defineEmits<{ pick: [id: string | null] }>()
</script>

<template>
  <button
    type="button"
    class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
    :aria-pressed="selected === null"
    @click="emit('pick', null)"
  >
    <Icon name="ok" :size="13" class="flex-none text-brand" :class="selected === null ? '' : 'invisible'" />
    <span
      class="flex size-[18px] flex-none items-center justify-center rounded-[5px] border border-dashed border-line"
    >
      <Icon name="close" :size="10" class="text-ink-3" />
    </span>
    <span
      class="min-w-0 truncate text-secondary"
      :class="selected === null ? 'font-semibold text-ink' : 'text-ink-2'"
      >{{ t('chat.newSession.noProject') }}</span
    >
  </button>
  <button
    v-for="p in state.projects"
    :key="p.id"
    type="button"
    class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
    :aria-pressed="p.id === selected"
    @click="emit('pick', p.id)"
  >
    <Icon name="ok" :size="13" class="flex-none text-brand" :class="p.id === selected ? '' : 'invisible'" />
    <ProjectSeal :project="p" :size="18" />
    <span
      class="min-w-0 truncate text-secondary"
      :class="p.id === selected ? 'font-semibold text-ink' : 'text-ink-2'"
      >{{ p.name }}</span
    >
    <span v-if="p.description" class="min-w-0 flex-1 truncate text-micro text-ink-3">{{
      p.description
    }}</span>
  </button>
</template>
