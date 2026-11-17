<script setup lang="ts">
// The projects page: the projects as a grid of cards and, below them, the
// workspace registry they mount from. A project opens at `/projects/<id>`, a
// workspace at `/workspaces/<name>`; `?new` on either is its creation form.
// At the desktop breakpoint it is a dialog over the grid; narrower, it takes
// the sheet's whole width with the header as the trail back.
import { computed, watch } from 'vue'
import { t } from '../../i18n'
import { layout } from '../../lib/layout'
import { SWAP } from '../../lib/motion'
import { openPage, pageUp } from '../../lib/router'
import { connected, refreshWorkspaces, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'
import PageSheet from './PageSheet.vue'
import ProjectCard from './ProjectCard.vue'
import ProjectForm from './ProjectForm.vue'
import WorkspaceForm from './WorkspaceForm.vue'

const route = computed(() => state.page!)
const onWorkspaces = computed(() => route.value.page === 'workspaces')

/** The open item's crumb, or none on the grid (or for a stale item). */
const crumb = computed(() => {
  const r = route.value
  if (r.fresh) return t(onWorkspaces.value ? 'workspaces.add' : 'projects.add')
  if (onWorkspaces.value) return state.workspaces.find((w) => w.name === r.item)?.name ?? null
  return state.projects.find((p) => p.id === r.item)?.name ?? null
})
/** The open item is a dialog over the grid rather than the sheet's body. */
const dialog = computed(() => layout.wide && !!crumb.value)
/** The item takes the sheet's body. */
const inSheet = computed(() => !layout.wide && !!crumb.value)

// A page booted from its address mounts before the socket is up.
watch(connected, (up) => up && void refreshWorkspaces(), { immediate: true })
</script>

<template>
  <PageSheet :title="t('projects.title')" icon="mount" tone="text-tool">
    <template #title>
      <template v-if="inSheet">
        <button type="button" class="flex-none text-ink-3 transition-colors hover:text-ink" @click="pageUp">
          {{ t('projects.title') }}
        </button>
        <Icon name="chevron" :size="14" class="-mx-1 flex-none text-ink-3" />
        <span class="min-w-0 truncate" :class="{ 'font-card': onWorkspaces && !route.fresh }">{{
          crumb
        }}</span>
      </template>
      <span v-else class="truncate">{{ t('projects.title') }}</span>
    </template>

    <Transition v-bind="SWAP" mode="out-in">
      <div v-if="inSheet" :key="`${route.page}/${route.item}`" class="flex min-h-0 flex-1 flex-col">
        <WorkspaceForm v-if="onWorkspaces" />
        <ProjectForm v-else />
      </div>

      <div v-else class="min-h-0 flex-1 overflow-y-auto px-4 pt-5 pb-8 sm:px-6">
        <div class="stagger grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-4">
          <ProjectCard
            v-for="(p, i) in state.projects"
            :key="p.id"
            :project="p"
            :style="{ '--i': i }"
            @open="openPage('projects', p.id)"
          />
          <button
            type="button"
            :style="{ '--i': state.projects.length }"
            class="flex min-h-36 items-center justify-center gap-2 rounded-card border border-dashed border-line text-secondary text-ink-3 transition-colors duration-200 hover:border-ink-3 hover:bg-surface-2/40 hover:text-ink"
            @click="openPage('projects', null, true)"
          >
            <Icon name="plus" :size="15" />
            {{ t('projects.add') }}
          </button>
        </div>

        <section class="mt-10" aria-labelledby="workspaces-heading">
          <h2 id="workspaces-heading" class="flex items-center gap-1.5 pb-2 text-micro font-bold text-ink-3">
            {{ t('workspaces.title') }}
            <span class="tabular-nums font-normal">{{ state.workspaces.length }}</span>
          </h2>
          <div class="grid grid-cols-[repeat(auto-fill,minmax(17rem,1fr))] gap-1">
            <button
              v-for="w in state.workspaces"
              :key="w.name"
              type="button"
              class="flex min-w-0 items-center gap-2.5 rounded-control px-2.5 py-2 text-left text-ink-2 transition-colors duration-150 hover:bg-surface-2/70 hover:text-ink"
              @click="openPage('workspaces', w.name)"
            >
              <Icon name="mount" :size="15" class="flex-none text-ink-3" />
              <span class="flex min-w-0 flex-col">
                <span class="truncate font-card text-secondary">{{ w.name }}</span>
                <span class="truncate font-card text-micro text-ink-3">{{
                  w.ssh ? `${w.ssh}:${w.root}` : w.root
                }}</span>
              </span>
            </button>
            <button
              type="button"
              class="flex items-center gap-2.5 rounded-control px-2.5 py-2 text-left text-secondary text-ink-3 transition-colors duration-150 hover:bg-surface-2/70 hover:text-ink"
              @click="openPage('workspaces', null, true)"
            >
              <Icon name="plus" :size="15" class="flex-none" />
              {{ t('workspaces.add') }}
            </button>
          </div>
        </section>
      </div>
    </Transition>

    <Modal
      :open="dialog"
      :label="crumb ?? ''"
      :z="55"
      wrapper-class="p-8"
      panel-class="max-h-full w-[min(36rem,100%)] rounded-sheet"
    >
      <PageSheet modal :title="crumb ?? ''" @close="pageUp">
        <template #title>
          <span class="truncate" :class="{ 'font-card': onWorkspaces && !route.fresh }">{{ crumb }}</span>
        </template>
        <Transition v-bind="SWAP" mode="out-in">
          <div :key="`${route.page}/${route.item}`" class="flex min-h-0 flex-1 flex-col">
            <WorkspaceForm v-if="onWorkspaces" />
            <ProjectForm v-else />
          </div>
        </Transition>
      </PageSheet>
    </Modal>
  </PageSheet>
</template>
