<script setup lang="ts">
// Right column: one structure, no tabs. Top to bottom:
//   Branches — the session tree.
//   Context  — the default workspace + the cards used on this path, with
//              the card library and workspace presets folded away.
//   Diagnostics — a pinned status pill (cache hit rate, hook count) that
//                 expands the cache meters and hook trace above it.
import { computed, ref } from 'vue'
import { t } from '../i18n'
import BranchTree from './BranchTree.vue'
import WorkspacesPanel from './WorkspacesPanel.vue'
import HookTracePanel from './HookTracePanel.vue'
import CachePanel from './CachePanel.vue'
import MemoryPanel from './MemoryPanel.vue'
import { branchTree, cacheTotal, hookTraceForSession, state } from '../lib/store'
import { INTERNAL_WORKSPACE } from '../lib/types'
import { layout } from '../lib/layout'
import Icon from './ui/Icon.vue'

const diagnosticsOpen = ref(false)

/** The line under `Context`: the branch's default workspace, compactly. */
const defaultWorkspace = computed(() => {
  const ps = state.pathState
  const name = ps?.default_workspace
  if (!ps || !name) return null
  if (name === INTERNAL_WORKSPACE)
    return { name, machine: t('panels.workspaces.virtual'), root: t('panels.context.virtualRoot') }
  const ws = ps.workspaces.find((w) => w.name === name)
  if (!ws) return { name, machine: '', root: '' }
  return { name, machine: ws.ssh ? `ssh:${ws.ssh}` : 'local', root: ws.root }
})

/** Presets plus the implicit `@rumu` workspace. */
const workspaceCount = computed(() => (state.pathState?.workspaces.length ?? 0) + 1)

const cachePct = computed(() => {
  const t = cacheTotal.value
  return t.prompt > 0 ? Math.round((t.cached * 100) / t.prompt) : 0
})
const hookCount = computed(() => hookTraceForSession.value.length)
</script>

<template>
  <!-- no `relative` here: the shell makes this `fixed` below xl, and
       position utilities would lose that to `relative` (stylesheet order). -->
  <aside class="flex min-h-0 min-w-0 flex-col px-3">
    <button
      class="icon-btn absolute top-2 right-3 z-10 size-8 xl:hidden"
      :title="t('chat.header.closeInspector')"
      :aria-label="t('chat.header.closeInspector')"
      @click="layout.inspectorOpen = false"
    >
      <Icon name="close" :size="13" />
    </button>

    <div class="-mx-3 flex flex-1 flex-col overflow-y-auto px-3 pt-3 pb-4">
      <div class="cascade flex flex-col gap-4">
        <BranchTree :items="branchTree" />

        <section>
          <div class="section-title mb-1.5">{{ t('panels.context.title') }}</div>
          <div
            v-if="defaultWorkspace"
            class="mb-2 flex items-center gap-1.5 rounded-card bg-surface px-3 py-1.5 shadow-soft"
            :title="`${defaultWorkspace.machine} · ${defaultWorkspace.root}`"
          >
            <Icon name="mount" :size="12" class="flex-none text-tool" />
            <span class="flex-none font-mono text-meta font-semibold text-tool"
              >* {{ defaultWorkspace.name }}</span
            >
            <span class="flex-none rounded-xs border border-line px-1 font-mono text-micro text-ink-3">
              {{ defaultWorkspace.machine }}
            </span>
            <span class="min-w-0 truncate font-mono text-micro text-ink-3">{{ defaultWorkspace.root }}</span>
          </div>

          <MemoryPanel />

          <details class="group mt-2">
            <summary
              class="flex cursor-pointer list-none items-center gap-1.5 rounded-card px-3 py-1.5 select-none hover:bg-surface-2/70 [&::-webkit-details-marker]:hidden"
            >
              <Icon name="chevron" :size="10" class="text-ink-3 transition-transform group-open:rotate-90" />
              <span class="text-meta font-semibold text-ink-2">
                {{ t('panels.context.workspaces', { count: workspaceCount }) }}
              </span>
            </summary>
            <div class="reveal mt-1">
              <WorkspacesPanel />
            </div>
          </details>
        </section>
      </div>
    </div>

    <div class="flex-none border-t border-line-soft pt-2 pb-3">
      <div
        v-if="diagnosticsOpen"
        class="mb-2 flex max-h-[48vh] flex-col gap-3 overflow-y-auto rounded-card bg-surface-2/40 p-2"
      >
        <div class="section-title">{{ t('panels.diagnostics.title') }}</div>
        <CachePanel />
        <HookTracePanel />
      </div>
      <button
        class="flex w-full items-center gap-2 rounded-full bg-surface-2 px-3 py-1.5 text-meta text-ink-2 transition-colors hover:text-ink"
        :aria-expanded="diagnosticsOpen"
        :title="t('panels.diagnostics.title')"
        @click="diagnosticsOpen = !diagnosticsOpen"
      >
        <Icon name="tool" :size="12" class="flex-none text-ink-3" />
        <span class="font-mono">{{ t('panels.diagnostics.cache', { pct: cachePct }) }}</span>
        <span class="text-ink-4">·</span>
        <span class="font-mono">{{ t('panels.diagnostics.hooks', { count: hookCount }) }}</span>
        <Icon
          name="chevron"
          :size="10"
          class="ml-auto flex-none text-ink-3 transition-transform"
          :class="{ 'rotate-90': diagnosticsOpen }"
        />
      </button>
    </div>
  </aside>
</template>
