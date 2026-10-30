<script setup lang="ts">
// Branches: the session's branch tree, drawn as one. Children hang off
// their parent's status dot through a trunk and a rounded elbow tinted by
// the child's kind (agent teal, revert amber).
import { t } from '../i18n'
import type { BranchTreeNode } from '../lib/store'
import { selectBranch, state } from '../lib/store'
import { preview, shortId } from '../lib/util'
import type { BranchStatus } from '../lib/types'

// Recursive: child branches render through `nested` instances. The heading
// is the inspector's.
withDefaults(defineProps<{ items: BranchTreeNode[]; nested?: boolean }>(), { nested: false })

type Tone = 'neutral' | 'pine' | 'agent' | 'revert'

function toneOf(b: BranchTreeNode['branch']): Tone {
  switch (b.kind.type) {
    case 'main':
      return 'pine'
    case 'agent':
      return 'agent'
    case 'revert':
      return 'revert'
    default:
      return 'neutral'
  }
}

// Literal utility strings (Tailwind only emits complete class names).
const toneClass: Record<Tone, string> = {
  neutral: 'text-ink-2',
  pine: 'text-pine',
  agent: 'text-agent',
  revert: 'text-revert',
}

const elbowTone: Record<Tone, string> = {
  neutral: 'border-ink-3/40',
  pine: 'border-line',
  agent: 'border-agent/55',
  revert: 'border-revert/55',
}

// Abandoned branches never reach the panel (see `branchTree`).
const statusDot: Partial<Record<BranchStatus, string>> = {
  active: 'bg-pine',
  done: 'bg-brand',
}

function desc(b: BranchTreeNode['branch']): string {
  const k = b.kind
  if (k.type === 'agent') {
    let out = preview(k.task, 60)
    if (k.inherit?.context === 'none') out += t('panels.branches.noContext')
    if (k.inherit?.workspaces?.length)
      out += t('panels.branches.scope', { names: k.inherit.workspaces.join(', ') })
    return out
  }
  if (k.type === 'revert') return k.lesson ? preview(k.lesson, 60) : ''
  return ''
}

const emit = defineEmits<{ picked: [] }>()

function onClick(id: string): void {
  void selectBranch(id)
  emit('picked')
}
</script>

<template>
  <div v-if="state.sessionId">
    <div class="flex flex-col" :class="{ 'relative ml-4 pl-2': nested }">
      <!-- the trunk, from the parent's dot down to the first elbow -->
      <span v-if="nested" aria-hidden="true" class="absolute -top-[11px] left-0 h-[11px] w-px bg-line" />
      <div v-for="(item, i) in items" :key="item.branch.id" class="relative">
        <template v-if="nested">
          <span
            v-if="i < items.length - 1"
            aria-hidden="true"
            class="absolute top-0 bottom-0 -left-2 w-px bg-line"
          />
          <span
            aria-hidden="true"
            class="absolute top-0 -left-2 h-[15px] w-[18px] rounded-bl-[8px] border-b border-l"
            :class="elbowTone[toneOf(item.branch)]"
          />
        </template>
        <button
          class="flex w-full items-center gap-1.5 rounded-card px-3 py-1.5 text-left transition-[background-color,box-shadow] duration-200"
          :class="
            item.branch.id === state.activeBranch ? 'bg-surface-2 font-semibold' : 'hover:bg-surface-2/60'
          "
          @click="onClick(item.branch.id)"
        >
          <span
            class="size-2 flex-none rounded-full"
            :class="statusDot[item.branch.status] ?? 'bg-ink-3'"
            :title="t(`panels.branches.status.${item.branch.status}`)"
          />
          <span class="flex-none text-meta font-bold" :class="toneClass[toneOf(item.branch)]">
            {{ t(`panels.branches.kind.${item.branch.kind.type}`) }}
          </span>
          <span class="min-w-0 truncate text-meta text-ink-2">{{ desc(item.branch) }}</span>
          <span class="ml-auto flex-none font-mono text-micro text-ink-3">{{ shortId(item.branch.id) }}</span>
        </button>
        <BranchTree v-if="item.children.length" :items="item.children" nested @picked="emit('picked')" />
      </div>
    </div>
  </div>
</template>
