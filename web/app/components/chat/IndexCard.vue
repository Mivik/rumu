<script setup lang="ts">
// One library card at 5:3, as an index card: the title above the coral
// header rule with the path filed at its right when it says more than the
// title, the description on the ruled lines, and the card's keywords as
// subject tags on the last one. Clicking opens it in the CardDrawer.
import { computed } from 'vue'
import type { Card } from '../../lib/types'
import { openDrawer } from '../../lib/store'

const props = defineProps<{ card: Card }>()

const keywords = computed(() => props.card.keywords ?? [])

/** Whether the path is filed beside the title. */
const filed = computed(() => !!props.card.title && props.card.title !== props.card.id)

/** Tags on the last ruled line; the rest become a `+N` badge. */
const SHOWN = 3
const tags = computed(() => keywords.value.slice(0, SHOWN))
const more = computed(() => keywords.value.length - tags.value.length)

/** The hover title carries the whole subject list. */
const tooltip = computed(() => {
  const base = props.card.description || props.card.title || props.card.id
  return keywords.value.length ? `${base}\n${keywords.value.join(' · ')}` : base
})
</script>

<template>
  <button
    type="button"
    class="index-card flex h-[120px] w-full min-w-0 flex-col pr-5 pl-3.5 text-left"
    :title="tooltip"
    @click="openDrawer(card.id)"
  >
    <span class="flex h-[30px] items-baseline gap-2 pt-[5px]">
      <span class="card-title min-w-0 flex-1 truncate text-secondary">{{ card.title || card.id }}</span>
      <span v-if="filed" class="max-w-[45%] flex-none truncate font-card text-micro text-ink-3 italic">{{
        card.id
      }}</span>
    </span>
    <span
      class="mt-[1.5px] font-card text-meta leading-[20px] text-ink-2"
      :class="keywords.length ? 'line-clamp-3 h-[60px]' : 'line-clamp-4 h-[80px]'"
      >{{ card.description }}</span
    >
    <span v-if="keywords.length" class="flex h-[20px] items-center gap-1.5 overflow-hidden">
      <span v-for="(k, i) in tags" :key="i" class="keyword-tag">{{ k }}</span>
      <span v-if="more > 0" class="flex-none text-micro text-ink-3 tabular-nums">+{{ more }}</span>
    </span>
  </button>
</template>
