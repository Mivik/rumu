<script setup lang="ts">
// A knowledge card on a tool row, set as a plain card reference (.card-ref)
// small enough to sit inside the row's line. Click → drawer.
import { openDrawer } from '../../lib/store'

const props = withDefaults(defineProps<{ id: string; useRef?: string; title?: string }>(), {
  useRef: undefined,
  title: undefined,
})

function onClick(): void {
  openDrawer(props.id, props.useRef)
}
</script>

<template>
  <button
    type="button"
    class="card-ref max-w-[180px] text-micro"
    :title="title ?? `#${id}`"
    @click.stop="onClick"
  >
    <span class="truncate">{{ id }}</span>
  </button>
</template>
