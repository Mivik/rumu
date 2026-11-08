<script setup lang="ts">
// The stack of open card windows, bottom first. Each is its own native
// dialog, so a window opened from a link sits over the card it was clicked in
// and closing it reveals that card again.
import { state } from '../../lib/store'
import CardWindow from './CardWindow.vue'
</script>

<template>
  <CardWindow v-for="entry in state.drawers" :key="entry.key" :entry="entry" />
</template>
