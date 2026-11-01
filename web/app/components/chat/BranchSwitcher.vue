<script setup lang="ts">
// Inline ‹ i/n › over the sibling subtrees of a message's parent: the
// replaced variants of a turn live here, not on the conversation line.
// Picking one swaps the session's active branch to it.
import { computed } from 'vue'
import { t } from '../../i18n'
import { isBusy, selectBranch, siblingAlternatives, state } from '../../lib/store'
import type { NodeId } from '../../lib/types'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ parentId: NodeId | null }>()

const alternatives = computed(() => siblingAlternatives(props.parentId))
const current = computed(() => alternatives.value.findIndex((a) => a.current))
const position = computed(() => Math.max(current.value, 0) + 1)

/** Step stops at the ends — variants are an ordered list, not a ring. */
function pick(index: number): void {
  if (isBusy.value) return
  const target = alternatives.value[index]
  if (!target || target.current || target.branch === state.activeBranch) return
  void selectBranch(target.branch)
}

function step(delta: number): void {
  pick(Math.max(current.value, 0) + delta)
}
</script>

<template>
  <span
    v-if="alternatives.length > 1"
    class="flex flex-none items-center gap-0.5 text-micro text-ink-3 tabular-nums"
  >
    <button
      class="icon-btn size-5"
      :title="t('chat.alternatives.previous')"
      :aria-label="t('chat.alternatives.previous')"
      :disabled="isBusy || current <= 0"
      @click="step(-1)"
    >
      <Icon name="chevron" :size="10" class="rotate-180" />
    </button>
    <span :title="t('chat.alternatives.title', { current: position, total: alternatives.length })">
      {{ position }}/{{ alternatives.length }}
    </span>
    <button
      class="icon-btn size-5"
      :title="t('chat.alternatives.next')"
      :aria-label="t('chat.alternatives.next')"
      :disabled="isBusy || current >= alternatives.length - 1"
      @click="step(1)"
    >
      <Icon name="chevron" :size="10" />
    </button>
  </span>
</template>
