<script setup lang="ts">
// One library card at 5:3, as an index card: the title above the coral
// header rule, the description on three ruled lines, the card's path filed
// on the last one. Clicking opens it in the CardDrawer.
import type { Card } from '../../lib/types'
import { openDrawer } from '../../lib/store'

defineProps<{ card: Card }>()
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
    <span class="mt-[1.5px] line-clamp-3 h-[60px] font-card text-meta leading-[20px] text-ink-2">{{
      card.description
    }}</span>
    <span class="truncate font-card text-micro leading-[20px] text-ink-3 italic">{{ card.id }}</span>
  </button>
</template>
