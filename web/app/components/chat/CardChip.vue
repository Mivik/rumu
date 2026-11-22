<script setup lang="ts">
// A knowledge card on a tool row or an edit's header, set where the path
// would be as a plain card reference (.card-ref). Click → drawer. Lifted
// 2px: its marker hangs below the text, and centered as a box the ink sat
// 3px low in the row.
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
  <button type="button" class="card-ref -top-0.5" :title="title ?? `#${id}`" @click.stop="onClick">
    <span class="truncate">{{ id }}</span>
  </button>
</template>
