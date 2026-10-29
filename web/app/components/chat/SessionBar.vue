<script setup lang="ts">
// The open session's controls, in the chat header. Two halves:
//   BranchSwitch — beside the title: which branch of the session you are
//     reading, opening the branch tree. Shown once there is more than one.
//   SessionTools — at the header's end: the cards this path has in play
//     (as index cards) and the diagnostics (cache meters, hook trace).
// Each opens a popover; nothing about the session takes a column of its own.
import { computed } from 'vue'
import { t } from '../../i18n'
import { activeCardIds } from '../../lib/cards'
import { branchTree, cacheTotal, currentBranch, state } from '../../lib/store'
import type { BranchTreeNode } from '../../lib/store'
import { preview } from '../../lib/util'
import BranchTree from '../BranchTree.vue'
import CachePanel from '../CachePanel.vue'
import HookTracePanel from '../HookTracePanel.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import PathCards from './PathCards.vue'

defineProps<{ part: 'branch' | 'tools' }>()

function countBranches(items: BranchTreeNode[]): number {
  return items.reduce((n, b) => n + 1 + countBranches(b.children), 0)
}
const branchCount = computed(() => countBranches(branchTree.value))

/** The open branch: its kind, tinted, and for a sub-agent what it was asked. */
const branch = computed(() => {
  const b = currentBranch.value
  if (!b) return null
  const k = b.kind
  const tone = k.type === 'agent' ? 'text-agent' : k.type === 'revert' ? 'text-revert' : 'text-pine'
  const desc =
    k.type === 'agent' ? preview(k.task, 40) : k.type === 'revert' ? preview(k.lesson ?? '', 40) : ''
  return { kind: t(`panels.branches.kind.${k.type}`), tone, desc }
})

const cardCount = computed(() => state.cards.filter((c) => activeCardIds.value.has(c.id)).length)

const cachePct = computed(() => {
  const c = cacheTotal.value
  return c.prompt > 0 ? Math.round((c.cached * 100) / c.prompt) : null
})
</script>

<template>
  <Popover
    v-if="part === 'branch' && branch && branchCount > 1"
    :label="t('panels.branches.title')"
    placement="bottom-start"
  >
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="bar-btn"
        :class="{ 'is-open': open }"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="fork" :size="14" class="text-ink-3" />
        <span class="flex-none" :class="branch.tone">{{ branch.kind }}</span>
        <span v-if="branch.desc" class="hidden min-w-0 truncate font-normal text-ink-3 md:inline">{{
          branch.desc
        }}</span>
        <span class="hidden flex-none font-normal text-ink-3 tabular-nums sm:inline">{{ branchCount }}</span>
        <Icon name="down" :size="12" class="text-ink-3" />
      </button>
    </template>
    <template #default="{ close }">
      <div class="panel-head">
        {{ t('panels.branches.title') }}
        <small>{{ branchCount }}</small>
      </div>
      <BranchTree :items="branchTree" @picked="close" />
    </template>
  </Popover>

  <span v-else-if="part === 'tools'" class="flex flex-none items-center gap-1">
    <Popover :label="t('cards.library.onPath')">
      <template #trigger="{ open, toggle }">
        <button
          type="button"
          class="bar-btn"
          :class="{ 'is-open': open }"
          :title="t('cards.library.onPath')"
          :aria-expanded="open"
          @click="toggle"
        >
          <Icon name="memory" :size="15" class="text-memory" />
          <span class="tabular-nums">{{ cardCount }}</span>
          <span class="hidden sm:inline">{{ t('chat.header.cards', { count: cardCount }) }}</span>
        </button>
      </template>
      <template #default="{ close }">
        <div class="panel-head">
          {{ t('cards.library.onPath') }}
          <small v-if="cardCount">{{ cardCount }}</small>
        </div>
        <PathCards @picked="close" />
      </template>
    </Popover>

    <Popover :label="t('panels.diagnostics.title')" wide>
      <template #trigger="{ open, toggle }">
        <button
          type="button"
          class="bar-btn"
          :class="{ 'is-open': open }"
          :title="t('panels.diagnostics.title')"
          :aria-label="t('panels.diagnostics.title')"
          :aria-expanded="open"
          @click="toggle"
        >
          <Icon name="gauge" :size="15" class="text-ink-3" />
          <span v-if="cachePct !== null" class="hidden tabular-nums sm:inline">{{
            t('panels.diagnostics.cache', { pct: cachePct })
          }}</span>
        </button>
      </template>
      <div class="panel-head">{{ t('panels.diagnostics.title') }}</div>
      <div class="flex flex-col gap-4">
        <CachePanel />
        <HookTracePanel />
      </div>
    </Popover>
  </span>
</template>
