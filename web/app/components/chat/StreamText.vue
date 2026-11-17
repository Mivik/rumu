<script setup lang="ts">
// The answer text of the live turn as it streams in, rendered as markdown.
// Its own component so a render touches this text alone: the group around it
// would otherwise re-render — and re-parse every finished text of the turn —
// on each one.
import { marked } from 'marked'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useBranchView } from '../../lib/store'
import { grammarVersion, renderMarkdown } from '../../lib/markdown'
import { pathImage } from '../../lib/artifacts'

const view = useBranchView()
const el = ref<HTMLElement | null>(null)

/** Blocks the text has moved past are drawn once and kept: only the tail
 * (the last two blocks, which more text can still reshape) is parsed,
 * sanitized and laid out again. Re-rendering the whole message instead
 * made every update cost as much as the answer was long, and a long answer
 * kept the main thread too busy to switch sessions. Markdown that reaches
 * back across blocks (reference links) shows once the reply lands. */
let settled = ''
let tail: ChildNode[] = []

function clear(): void {
  el.value?.replaceChildren()
  settled = ''
  tail = []
}

function append(src: string): ChildNode[] {
  const t = document.createElement('template')
  t.innerHTML = renderMarkdown(src, { image: (a) => pathImage(a) })
  const nodes = [...t.content.childNodes]
  el.value!.append(t.content)
  return nodes
}

function draw(text: string): void {
  const box = el.value
  if (!box) return
  if (!text.startsWith(settled)) clear()
  const rest = text.slice(settled.length)
  const tokens = marked.lexer(rest)
  const blocks = tokens.flatMap((tk, i) => (tk.type === 'space' ? [] : [i]))
  const done = tokens
    .slice(0, blocks.at(-2) ?? 0)
    .map((tk) => tk.raw)
    .join('')
  for (const n of tail) n.remove()
  if (done && rest.startsWith(done)) {
    append(done)
    settled += done
  }
  tail = append(text.slice(settled.length))
}

/** Updates are also coalesced to one every ~50 ms, however fast the deltas. */
const current = () => view.delta.value?.text ?? ''
let shownAt = performance.now()
let timer: ReturnType<typeof setTimeout> | null = null

function flush(): void {
  timer = null
  shownAt = performance.now()
  draw(current())
}

watch(current, () => {
  if (timer === null) timer = setTimeout(flush, Math.max(0, 50 - (performance.now() - shownAt)))
})

// A highlighting grammar landed: kept blocks may hold a fence it colors.
watch(grammarVersion, () => {
  clear()
  draw(current())
})

onMounted(() => draw(current()))

onBeforeUnmount(() => {
  if (timer !== null) clearTimeout(timer)
})
</script>

<template>
  <div ref="el" class="md streaming-caret" />
</template>
