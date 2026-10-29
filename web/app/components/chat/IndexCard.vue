<script setup lang="ts">
// One library card at 5:3, as an index card: the title above the coral
// header rule, the description on the ruled lines, and the card's path filed
// on the last one when it says more than the title — otherwise the
// description keeps that line. Clicking opens it in the CardDrawer.
import { computed } from 'vue'
import type { Card } from '../../lib/types'
import { openDrawer } from '../../lib/store'

const props = defineProps<{ card: Card }>()

/** Whether the path is filed on the last ruled line. */
const filed = computed(() => !!props.card.title && props.card.title !== props.card.id)
</script>

<template>
  <button
    type="button"
    class="index-card flex h-[120px] w-full min-w-0 flex-col pr-5 pl-3.5 text-left"
    :title="card.description || card.title || card.id"
    @click="openDrawer(card.id)"
  >
    <span class="card-title block h-[30px] truncate pt-[5px] text-secondary">{{
      card.title || card.id
    }}</span>
    <span
      class="mt-[1.5px] font-card text-meta leading-[20px] text-ink-2"
      :class="filed ? 'line-clamp-3 h-[60px]' : 'line-clamp-4 h-[80px]'"
      >{{ card.description }}</span
    >
    <span
      v-if="filed"
      class="truncate font-card text-micro leading-[20px] text-ink-3 italic"
      >{{ card.id }}</span
    >
  </button>
</template>
