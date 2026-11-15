<script setup lang="ts">
// The landing composer's two picks: the assistant a new session runs as and
// the project it works in. Both are single choice; "No project" is a choice.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import {
  chooseLandingAssistant,
  chooseLandingProject,
  landingAssistant,
  landingProject,
  state,
} from '../../lib/store'
import { assistantCat } from '../../lib/assistantFace'
import AssistantAvatar from '../brand/AssistantAvatar.vue'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import AssistantMenu from './AssistantMenu.vue'

const assistant = computed(() => state.assistants.find((a) => a.id === landingAssistant.value) ?? null)
const project = computed(() => state.projects.find((p) => p.id === landingProject.value) ?? null)
/** The pointer is on the assistant's pick: its cat turns to face it. */
const near = ref(false)

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
        class="inline-flex min-w-0 items-center gap-2 rounded-[11px] bg-surface-2 p-[3px] pr-2.5 text-secondary font-semibold text-ink-2 transition-colors hover:text-ink"
        :class="{ 'text-ink': open }"
        :aria-expanded="open"
        @click="toggle"
        @pointerenter="near = true"
        @pointerleave="near = false"
      >
        <AssistantAvatar
          v-if="assistant"
          :key="assistant.id"
          :id="assistant.id"
          :cat="assistantCat(assistant)"
          :size="24"
          :near="near || open"
        />
        <span class="truncate">{{ assistant?.name ?? t('chat.newSession.pickAssistant') }}</span>
        <Icon name="down" :size="12" class="text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <AssistantMenu :current="landingAssistant" @pick="pickAssistant($event, close)" @close="close" />
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
        <ProjectSeal v-if="project" :project="project" :size="16" />
        <Icon v-else name="mount" :size="14" class="text-ink-3" />
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
          class="flex size-[18px] flex-none items-center justify-center rounded-[5px] border border-dashed border-line"
        >
          <Icon name="close" :size="10" class="text-ink-3" />
        </span>
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
        <ProjectSeal :project="p" :size="18" />
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
