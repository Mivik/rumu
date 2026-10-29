<script setup lang="ts">
// A knowledge card, everywhere: same icon, same memory tone, click → drawer.
import type { Card } from '../../lib/types'
import Chip from '../ui/Chip.vue'
import { openDrawer } from '../../lib/store'

const props = withDefaults(defineProps<{ card: Card; useNodeId?: string; title?: string }>(), {
  useNodeId: undefined,
  title: undefined,
})

function onClick(): void {
  openDrawer(props.card.id, props.useNodeId)
}

function tooltip(): string {
  return `#${props.card.id}${props.card.description ? ` — ${props.card.description}` : ''}`
}
</script>

<template>
  <Chip
    icon="memory"
    tone="memory"
    :label="card.title || card.id"
    :title="title ?? tooltip()"
    clickable
    @click="onClick"
  />
</template>
