<script setup lang="ts">
// The projects page: the projects and, below them, the workspace registry
// they mount from. A project opens at `/projects/<id>`, a workspace at
// `/workspaces/<name>`; `?new` on either is its creation form.
import { computed, watch } from 'vue'
import { t } from '../../i18n'
import { openPage, pageUp } from '../../lib/router'
import { connected, refreshWorkspaces, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'
import PageSheet from './PageSheet.vue'
import ProjectForm from './ProjectForm.vue'
import WorkspaceForm from './WorkspaceForm.vue'

const route = computed(() => state.page!)
const onWorkspaces = computed(() => route.value.page === 'workspaces')

/** Whether `page`'s item (or its creation form) is the one open. */
function isOpen(page: 'projects' | 'workspaces', item: string | null, fresh = false): boolean {
  const r = route.value
  return r.page === page && r.fresh === fresh && (fresh || r.item === item)
}

const detail = computed(() => {
  const r = route.value
  if (r.fresh) return true
  return onWorkspaces.value
    ? state.workspaces.some((w) => w.name === r.item)
    : state.projects.some((p) => p.id === r.item)
})

const ROW = 'flex flex-col rounded-control px-2.5 py-2 text-left transition-colors duration-150'
const ADD =
  'mt-1 flex items-center gap-2.5 rounded-control border border-dashed px-2.5 py-2 text-left text-secondary transition-colors duration-150'

function rowTone(open: boolean): string {
  return open
    ? 'bg-surface font-semibold text-ink shadow-soft'
    : 'text-ink-2 hover:bg-surface/70 hover:text-ink'
}

function addTone(open: boolean): string {
  return open
    ? 'border-brand/50 bg-brand-soft/50 font-semibold text-ink'
    : 'border-line text-ink-3 hover:border-ink-3 hover:text-ink'
}

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshWorkspaces(), { immediate: true })
</script>

<template>
  <PageSheet
    :title="t('projects.title')"
    icon="mount"
    tone="text-tool"
    :detail="detail"
    :pick="t('projects.pick')"
    @back="pageUp"
  >
    <template #list>
      <div class="flex min-h-0 flex-1 flex-col overflow-y-auto p-3">
        <section class="flex flex-col gap-0.5" :aria-label="t('projects.title')">
          <button
            v-for="p in state.projects"
            :key="p.id"
            type="button"
            :class="[ROW, rowTone(isOpen('projects', p.id))]"
            @click="openPage('projects', p.id)"
          >
            <span class="truncate text-secondary">{{ p.name }}</span>
            <span class="truncate font-card text-micro font-normal text-ink-3">{{
              p.workspaces.join(' · ') || t('projects.noWorkspaces')
            }}</span>
          </button>
          <button
            type="button"
            :class="[ADD, addTone(isOpen('projects', null, true))]"
            @click="openPage('projects', null, true)"
          >
            <Icon name="plus" :size="14" />
            {{ t('projects.add') }}
          </button>
        </section>

        <section class="mt-6 flex flex-col gap-0.5" :aria-labelledby="'workspaces-heading'">
          <h2
            id="workspaces-heading"
            class="flex items-center gap-1.5 px-2.5 pb-1 text-micro font-bold text-ink-3"
          >
            {{ t('workspaces.title') }}
            <span class="tabular-nums font-normal">{{ state.workspaces.length }}</span>
          </h2>
          <button
            v-for="w in state.workspaces"
            :key="w.name"
            type="button"
            :class="[ROW, rowTone(isOpen('workspaces', w.name))]"
            @click="openPage('workspaces', w.name)"
          >
            <span class="truncate font-card text-secondary">{{ w.name }}</span>
            <span class="truncate font-card text-micro font-normal text-ink-3">{{
              w.ssh ? `${w.ssh}:${w.root}` : w.root
            }}</span>
          </button>
          <button
            type="button"
            :class="[ADD, addTone(isOpen('workspaces', null, true))]"
            @click="openPage('workspaces', null, true)"
          >
            <Icon name="plus" :size="14" />
            {{ t('workspaces.add') }}
          </button>
        </section>
      </div>
    </template>

    <WorkspaceForm v-if="onWorkspaces" />
    <ProjectForm v-else />
  </PageSheet>
</template>
