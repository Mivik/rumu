<script setup lang="ts">
// The composer's workspaces (the open branch's, or the new session's on the
// landing page): pick the default, remove one, add a registered one, or open
// the registry. `@rumu` is always there for the model and never listed.
import { computed } from 'vue'
import { t } from '../i18n'
import { openWorkspaceSettings } from '../lib/layout'
import { attachWorkspaces, composerWorkspaces, state, useWorkspace } from '../lib/store'
import type { Workspace } from '../lib/types'
import Icon from './ui/Icon.vue'

const emit = defineEmits<{ close: [] }>()

const attached = computed(() => composerWorkspaces.value?.workspaces ?? [])
const names = computed(() => attached.value.map((w) => w.name))
const others = computed(() => state.workspaces.filter((w) => !names.value.includes(w.name)))
const current = computed(() => composerWorkspaces.value?.default ?? null)

function where(w: Workspace): string {
  return w.ssh ? `${w.ssh}:${w.root}` : w.root
}

function detach(name: string): void {
  void attachWorkspaces(names.value.filter((n) => n !== name))
}

function attach(name: string): void {
  void attachWorkspaces([...names.value, name])
}

function edit(name = ''): void {
  emit('close')
  openWorkspaceSettings(name, !name)
}
</script>

<template>
  <div class="flex flex-col gap-0.5">
    <div
      v-for="w in attached"
      :key="w.name"
      class="group flex items-center gap-1 rounded-control transition-colors duration-150 hover:bg-surface-2"
    >
      <button
        type="button"
        class="flex min-w-0 flex-1 items-center gap-2 py-1.5 pl-2.5 text-left"
        :title="t('workspaces.makeDefault')"
        :aria-pressed="w.name === current"
        @click="useWorkspace(w.name)"
      >
        <Icon
          name="ok"
          :size="13"
          class="flex-none text-brand"
          :class="w.name === current ? '' : 'invisible'"
        />
        <span
          class="flex-none font-card text-secondary"
          :class="w.name === current ? 'font-semibold text-ink' : 'text-ink-2'"
          >{{ w.name }}</span
        >
        <span class="min-w-0 truncate font-card text-meta text-ink-3" :title="where(w)">{{ where(w) }}</span>
        <span v-if="w.read_only" class="flex-none rounded-xs border border-line px-1 text-micro text-ink-3">{{
          t('workspaces.readOnly')
        }}</span>
      </button>
      <button
        type="button"
        class="icon-btn size-6 flex-none opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
        :title="t('workspaces.manage')"
        :aria-label="t('workspaces.manage')"
        @click="edit(w.name)"
      >
        <Icon name="edit" :size="12" />
      </button>
      <button
        type="button"
        class="icon-btn mr-1 size-6 flex-none"
        :title="t('workspaces.detach')"
        :aria-label="t('workspaces.detach')"
        @click="detach(w.name)"
      >
        <Icon name="close" :size="12" />
      </button>
    </div>

    <div v-if="attached.length && others.length" class="mx-2 my-1 border-t border-line-soft" />
    <button
      v-for="w in others"
      :key="w.name"
      type="button"
      class="flex items-center gap-2 rounded-control py-1.5 pl-2.5 pr-2 text-left text-ink-3 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
      :title="t('workspaces.attach')"
      @click="attach(w.name)"
    >
      <Icon name="plus" :size="13" class="flex-none" />
      <span class="flex-none font-card text-secondary">{{ w.name }}</span>
      <span class="min-w-0 truncate font-card text-meta" :title="where(w)">{{ where(w) }}</span>
    </button>

    <div class="mx-2 my-1 border-t border-line-soft" />
    <button
      type="button"
      class="flex items-center gap-2 rounded-control py-1.5 pl-2.5 pr-2 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
      @click="edit()"
    >
      <Icon name="plus" :size="13" class="flex-none" />
      {{ t('workspaces.add') }}
    </button>
  </div>
</template>
