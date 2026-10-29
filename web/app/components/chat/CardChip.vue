<script setup lang="ts">
// A knowledge card on a tool row: the same little card stub as a recalled
// card in the timeline (.card-tab), set small enough to sit inside the row's
// line. Click → drawer.
import type { Card } from '../../lib/types'
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
  <button
    type="button"
    class="card-tab max-w-[180px] border-t-[1.5px] px-1.5 pt-[2px] pb-[3px] text-micro leading-none"
    :title="title ?? tooltip()"
    @click.stop="onClick"
  >
    <span class="truncate">{{ card.title || card.id }}</span>
  </button>
</template>
