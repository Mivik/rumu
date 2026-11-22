<script setup lang="ts">
// The answer text of the live turn as it streams in, rendered as markdown.
// Its own component so a render touches this text alone: the group around it
// would otherwise re-render — and re-parse every finished text of the turn —
// on each one.
import { marked, type Tokens } from 'marked'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useBranchView } from '../../lib/store'
import { assetsVersion, fenceClosed, renderMarkdown } from '../../lib/markdown'
import { imagesOnPath, pathImage } from '../../lib/artifacts'
import { isWidgetFence } from '../../lib/widgets'

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

/** A fence still open at the end of the text grows as plain text appended
 * to its code element, highlighted once it closes: re-highlighting a long
 * block on every update would cost as much as the block is long. */
let open: { at: number; code: Text } | null = null

function clear(): void {
  el.value?.replaceChildren()
  settled = ''
  tail = []
  open = null
}

function nodesOf(src: string): ChildNode[] {
  const t = document.createElement('template')
  t.innerHTML = renderMarkdown(src, { image: (a) => pathImage(a), widgets: true })
  return [...t.content.childNodes]
}

/** The opening line and fence of a fenced code block not closed yet. */
function openFence(tk: Tokens.Code): { line: string; fence: string } | null {
  const lines = tk.raw.trimEnd().split('\n')
  const line = lines[0]!
  const fence = /^ {0,3}(`{3,}|~{3,})/.exec(line)?.[1]
  if (!fence) return null
  const close = new RegExp(`^ {0,3}${fence[0]}{${fence.length},}[ \\t]*$`)
  return lines.length > 1 && close.test(lines.at(-1)!) ? null : { line, fence }
}

function draw(text: string): void {
  const box = el.value
  if (!box) return
  if (!text.startsWith(settled)) clear()
  const rest = text.slice(settled.length)
  const tokens = marked.lexer(rest)
  const blocks = tokens.flatMap((tk, i) => (tk.type === 'space' ? [] : [i]))
  const lastTk = tokens[blocks.at(-1) ?? -1]
  const last = lastTk?.type === 'code' && rest.endsWith(lastTk.raw) ? (lastTk as Tokens.Code) : null
  const fence = last && openFence(last)
  const at = last && fence ? text.length - last.raw.length : -1
  if (last && fence && open?.at === at && last.text.startsWith(open.code.data)) {
    open.code.appendData(last.text.slice(open.code.data.length))
    return
  }

  for (const n of tail) n.remove()
  open = null
  // A widget is kept as soon as its fence closes: drawing it again would
  // reload its page on every update.
  const widget = tokens.findLastIndex(
    (tk) => tk.type === 'code' && isWidgetFence((tk as Tokens.Code).lang) && fenceClosed(tk.raw),
  )
  const done = tokens
    .slice(0, Math.max(blocks.at(-2) ?? 0, widget + 1))
    .map((tk) => tk.raw)
    .join('')
  if (done && rest.startsWith(done)) {
    box.append(...nodesOf(done))
    settled += done
  }
  if (last && fence) {
    const live = text.slice(settled.length, at)
    const shell = nodesOf(`${fence.line}\n${fence.fence}`)
    const code = shell.find((n): n is Element => n instanceof Element)?.querySelector('code')
    if (code) {
      open = { at, code: document.createTextNode(last.text) }
      code.replaceChildren(open.code)
      tail = [...nodesOf(live), ...shell]
    }
  }
  if (!open) tail = nodesOf(text.slice(settled.length))
  box.append(...tail)
}

/** Updates are coalesced: at least 50 ms apart, and three times as far as
 * the last draw took, so drawing keeps at most a quarter of the main thread
 * whatever the text holds and however slow the machine. */
const current = () => view.delta.value?.text ?? ''
let shownAt = performance.now()
let gap = 50
let timer: ReturnType<typeof setTimeout> | null = null

function flush(): void {
  timer = null
  shownAt = performance.now()
  draw(current())
  gap = Math.max(50, 3 * (performance.now() - shownAt))
}

watch(current, () => {
  if (timer === null) timer = setTimeout(flush, Math.max(0, gap - (performance.now() - shownAt)))
})

// A grammar or KaTeX landed, or an image the text names: kept blocks may
// render differently now.
watch([assetsVersion, imagesOnPath], () => {
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
