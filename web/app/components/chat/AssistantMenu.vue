<script setup lang="ts">
// The assistant picker's panel: every assistant by its avatar and name, the
// current one checked, then the way to the assistants page.
import { t } from '../../i18n'
import { openPage } from '../../lib/router'
import { state } from '../../lib/store'
import { assistantCat } from '../../lib/assistantFace'
import AssistantAvatar from '../brand/AssistantAvatar.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ current: string }>()
const emit = defineEmits<{ pick: [id: string]; close: [] }>()

function manage(): void {
  emit('close')
  openPage('assistants', props.current)
}
</script>

<template>
  <div class="panel-head">{{ t('chat.newSession.assistant') }}</div>
  <button
    v-for="a in state.assistants"
    :key="a.id"
    type="button"
    class="flex w-full items-center gap-2.5 rounded-control px-2 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
    :aria-pressed="a.id === current"
    @click="a.id === current ? emit('close') : emit('pick', a.id)"
  >
    <AssistantAvatar :id="a.id" :cat="assistantCat(a)" :size="30" />
    <span
      class="min-w-0 truncate text-secondary"
      :class="a.id === current ? 'font-semibold text-ink' : 'text-ink-2'"
      >{{ a.name }}</span
    >
    <span v-if="a.default" class="flex-none rounded-xs border border-line px-1 text-micro text-ink-3">{{
      t('assistants.defaultBadge')
    }}</span>
    <span v-if="a.id === current" class="ml-auto flex-none text-brand"><Icon name="ok" :size="14" /></span>
  </button>
  <div class="mx-2 my-1 border-t border-line-soft" />
  <button
    type="button"
    class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
    @click="manage"
  >
    <Icon name="settings" :size="13" />
    {{ t('assistants.manage') }}
  </button>
</template>
