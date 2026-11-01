<script setup lang="ts">
// A knowledge entry: id, summary, then keywords.
import { computed } from 'vue'
import type { Card } from '../../lib/types'
import { openDrawer } from '../../lib/store'

const props = defineProps<{ card: Card }>()
const keywords = computed(() => props.card.keywords ?? [])
const tags = computed(() => keywords.value.slice(0, 3))
const more = computed(() => keywords.value.length - tags.value.length)
const tooltip = computed(() => {
  const base = props.card.description || props.card.id
  return keywords.value.length ? `${base}\n${keywords.value.join(' · ')}` : base
})
</script>

<template>
  <button
    type="button"
    class="card-entry flex w-full min-w-0 flex-col p-4 text-left hover:border-memory/40"
    :title="tooltip"
    @click="openDrawer(card.id)"
  >
    <span class="line-clamp-2 break-anywhere font-card text-[16px] leading-snug font-semibold text-ink">{{
      card.id
    }}</span>
    <span v-if="card.description" class="mt-1.5 line-clamp-2 text-[14px] leading-relaxed text-ink-2">{{
      card.description
    }}</span>
    <span v-if="keywords.length" class="mt-2.5 flex min-w-0 max-w-full items-center gap-1.5">
      <span v-for="(k, i) in tags" :key="i" class="keyword-tag">{{ k }}</span>
      <span v-if="more > 0" class="flex-none text-meta text-ink-3 tabular-nums">+{{ more }}</span>
    </span>
  </button>
</template>
