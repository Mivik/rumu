<script setup lang="ts">
// The answer text of the live turn as it streams in, rendered as markdown.
// Its own component so a render touches this text alone: the group around it
// would otherwise re-render — and re-parse every finished text of the turn —
// on each one.
import { onBeforeUnmount, ref, watch } from 'vue'
import { useBranchView } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import { pathImage } from '../../lib/artifacts'

const view = useBranchView()

/** The text as it is actually rendered: coalesced so a burst of deltas costs
 * at most one markdown parse every ~50 ms instead of one per delta. Each
 * parse is marked + DOMPurify over the *whole* message so far, and doing
 * that per delta kept the main thread from answering clicks while a long
 * answer streamed. */
const current = () => view.delta.value?.text ?? ''
const shown = ref(current())
let shownAt = performance.now()
let timer: ReturnType<typeof setTimeout> | null = null

function flush(): void {
  timer = null
  shownAt = performance.now()
  shown.value = current()
}

watch(current, () => {
  if (timer === null) timer = setTimeout(flush, Math.max(0, 50 - (performance.now() - shownAt)))
})

onBeforeUnmount(() => {
  if (timer !== null) clearTimeout(timer)
})
</script>

<template>
  <div class="md streaming-caret" v-html="renderMarkdown(shown, { image: (a) => pathImage(a) })" />
</template>
