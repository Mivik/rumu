<script setup lang="ts">
// A knowledge card on a tool row, set as a plain card reference (.card-ref)
// small enough to sit inside the row's line. Click → drawer.
import type { Card } from '../../lib/types'
import { openDrawer } from '../../lib/store'

const props = withDefaults(defineProps<{ card: Card; useRef?: string; title?: string }>(), {
  useRef: undefined,
  title: undefined,
})

function onClick(): void {
  openDrawer(props.card.id, props.useRef)
}

function tooltip(): string {
  return `#${props.card.id}${props.card.description ? ` — ${props.card.description}` : ''}`
}
</script>

<template>
  <button
    type="button"
    class="card-ref max-w-[180px] text-micro"
    :title="title ?? tooltip()"
    @click.stop="onClick"
  >
    <span class="truncate">{{ card.id }}</span>
  </button>
</template>
