<script setup lang="ts">
// Workspaces on the current branch path: the user presets from
// config.yaml (available to every session) plus the implicit `@rumu`
// workspace holding rumu's own data, with the default marker.
// Presets are user config — the panel is read-only; the model switches
// defaults with `workspace_use`.
import { computed } from 'vue'
import { t } from '../i18n'
import { state } from '../lib/store'
import { INTERNAL_WORKSPACE } from '../lib/types'
import Icon from './ui/Icon.vue'

interface Row {
  name: string
  /** Machine label: `local` or `ssh:<host>`. */
  machine: string
  root: string
  read_only: boolean
  isDefault: boolean
}

const rows = computed<Row[]>(() => {
  const ws = state.pathState?.workspaces ?? []
  const def = state.pathState?.default_workspace ?? null
  return ws.map((w) => ({
    name: w.name,
    machine: w.ssh ? `ssh:${w.ssh}` : 'local',
    root: w.root,
    read_only: w.read_only,
    isDefault: def === w.name,
  }))
})

const rumuDefault = computed(() => state.pathState?.default_workspace === INTERNAL_WORKSPACE)
</script>

<template>
  <div>
    <div class="flex flex-col gap-1.5">
      <div
        v-for="w in rows"
        :key="w.name"
        class="flex items-center gap-2 rounded-card bg-surface px-3 py-2 shadow-soft"
        :title="`${w.machine} · ${w.root}`"
      >
        <Icon name="mount" :size="13" class="flex-none text-tool" />
        <span
          v-if="w.isDefault"
          class="flex-none font-mono font-semibold text-tool"
          :title="t('panels.workspaces.default')"
          >*</span
        >
        <span class="flex-none font-mono text-secondary text-tool">{{ w.name }}</span>
        <span class="flex-none rounded-xs border border-line px-1 font-mono text-micro text-ink-3">{{
          w.machine
        }}</span>
        <span class="min-w-0 truncate font-mono text-meta text-ink-2">{{ w.root }}</span>
        <span
          v-if="w.read_only"
          class="flex-none rounded-xs border border-line px-1 font-mono text-micro text-ink-3"
        >
          {{ t('panels.workspaces.readOnly') }}
        </span>
      </div>
      <div class="flex items-center gap-2 rounded-card bg-surface px-3 py-2 shadow-soft">
        <Icon name="memory" :size="13" class="flex-none text-memory" />
        <span
          v-if="rumuDefault"
          class="flex-none font-mono font-semibold text-tool"
          :title="t('panels.workspaces.default')"
          >*</span
        >
        <span class="flex-none font-mono text-secondary text-memory">{{ INTERNAL_WORKSPACE }}</span>
        <span class="flex-none rounded-xs border border-line px-1 font-mono text-micro text-ink-3">{{
          t('panels.workspaces.virtual')
        }}</span>
        <span class="min-w-0 truncate font-mono text-meta text-ink-2">{{
          t('panels.context.virtualRoot')
        }}</span>
      </div>
    </div>
  </div>
</template>
