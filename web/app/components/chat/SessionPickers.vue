<script setup lang="ts">
// The landing composer's two picks: the assistant a new session runs as and
// the project it works in. Both are single choice; "No project" is a choice.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openAssistantSettings } from '../../lib/layout'
import {
  chooseLandingAssistant,
  chooseLandingProject,
  landingAssistant,
  landingProject,
  state,
} from '../../lib/store'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const assistant = computed(() => state.assistants.find((a) => a.id === landingAssistant.value) ?? null)
const project = computed(() => state.projects.find((p) => p.id === landingProject.value) ?? null)

function manage(close: () => void): void {
  close()
  openAssistantSettings()
}

function pickAssistant(id: string, close: () => void): void {
  chooseLandingAssistant(id)
  close()
}

function pickProject(id: string | null, close: () => void): void {
  chooseLandingProject(id)
  close()
}
</script>

<template>
  <Popover :label="t('chat.newSession.assistant')" placement="top-start">
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="inline-flex min-w-0 items-center gap-1.5 rounded-full bg-surface-2 py-1 pr-2 pl-2.5 text-meta font-semibold text-ink-2 transition-colors hover:text-ink"
        :class="{ 'text-ink': open }"
        :aria-expanded="open"
        @click="toggle"
      >
        <RumuCat v-if="assistant" :size="14" :paper="false" />
        <span class="truncate">{{ assistant?.name ?? t('chat.newSession.pickAssistant') }}</span>
        <Icon name="down" :size="12" class="text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="panel-head">{{ t('chat.newSession.assistant') }}</div>
      <button
        v-for="a in state.assistants"
        :key="a.id"
        type="button"
        class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
        :aria-pressed="a.id === landingAssistant"
        @click="pickAssistant(a.id, close)"
      >
        <Icon
          name="ok"
          :size="13"
          class="flex-none text-brand"
          :class="a.id === landingAssistant ? '' : 'invisible'"
        />
        <RumuCat :size="16" :paper="false" />
        <span
          class="min-w-0 truncate text-secondary"
          :class="a.id === landingAssistant ? 'font-semibold text-ink' : 'text-ink-2'"
          >{{ a.name }}</span
        >
        <span
          v-if="a.default"
          class="ml-auto flex-none rounded-xs border border-line px-1 text-micro text-ink-3"
          >{{ t('assistants.defaultBadge') }}</span
        >
      </button>
      <div class="mx-2 my-1 border-t border-line-soft" />
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-meta text-ink-3 transition-colors hover:bg-surface-2 hover:text-ink"
        @click="manage(close)"
      >
        <Icon name="settings" :size="13" />
        {{ t('assistants.manage') }}
      </button>
    </template>
  </Popover>

  <Popover :label="t('chat.newSession.project')" placement="top-start">
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="inline-flex min-w-0 items-center gap-1.5 rounded-full bg-surface-2 py-1 pr-2 pl-2.5 text-meta font-semibold text-ink-2 transition-colors hover:text-ink"
        :class="{ 'text-ink': open }"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="mount" :size="14" class="text-tool" />
        <span class="truncate">{{ project?.name ?? t('chat.newSession.noProject') }}</span>
        <Icon name="down" :size="12" class="text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="panel-head">{{ t('chat.newSession.project') }}</div>
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
        :aria-pressed="landingProject === null"
        @click="pickProject(null, close)"
      >
        <Icon
          name="ok"
          :size="13"
          class="flex-none text-brand"
          :class="landingProject === null ? '' : 'invisible'"
        />
        <span
          class="min-w-0 truncate text-secondary"
          :class="landingProject === null ? 'font-semibold text-ink' : 'text-ink-2'"
          >{{ t('chat.newSession.noProject') }}</span
        >
      </button>
      <button
        v-for="p in state.projects"
        :key="p.id"
        type="button"
        class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left transition-colors duration-150 hover:bg-surface-2"
        :aria-pressed="p.id === landingProject"
        @click="pickProject(p.id, close)"
      >
        <Icon
          name="ok"
          :size="13"
          class="flex-none text-brand"
          :class="p.id === landingProject ? '' : 'invisible'"
        />
        <span
          class="min-w-0 truncate text-secondary"
          :class="p.id === landingProject ? 'font-semibold text-ink' : 'text-ink-2'"
          >{{ p.name }}</span
        >
        <span v-if="p.description" class="min-w-0 flex-1 truncate text-micro text-ink-3">{{
          p.description
        }}</span>
      </button>
    </template>
  </Popover>
</template>
