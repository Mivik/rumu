<script setup lang="ts">
// The passage selected in the conversation right now, with its two uses —
// quote it into the message, or ask about it on the side (SidePop). Web
// pages cannot add to the browser's own selection menu, so the choice floats
// just above the composer, clear of the phone's selection toolbar and
// handles. It floats over the conversation rather than taking room: a page
// followed to its end would otherwise ride up under the selection.
import { onBeforeUnmount, onMounted } from 'vue'
import { t } from '../../i18n'
import type { Quote } from '../../lib/types'
import { addQuote, openSide, selection } from '../../lib/state/side'
import { requestComposerFocus } from '../../lib/state/ui'
import Icon from '../ui/Icon.vue'

/** The selection, when it lies inside one reply of the conversation. */
function read(): Quote | null {
  const sel = getSelection()
  if (!sel || sel.isCollapsed || !sel.rangeCount) return null
  const text = sel.toString().trim()
  if (!text) return null
  const r = sel.getRangeAt(0)
  const reply = (n: globalThis.Node): Element | null | undefined =>
    (n instanceof Element ? n : n.parentElement)?.closest('.tl-col .md[data-path-node]')
  const at = reply(r.startContainer)
  const node = at?.getAttribute('data-path-node')
  if (!at || !node || at !== reply(r.endContainer)) return null
  return { node, text }
}

// A phone's selection settles only once its handles are let go, and a press
// on a key collapses it before the click lands: the reading waits a beat,
// and a press freezes what it saw.
let timer = 0
let frozen: Quote | null = null

function onSelectionChange(): void {
  clearTimeout(timer)
  timer = window.setTimeout(() => {
    const q = read()
    if (q || !frozen) selection.value = q
  }, 200)
}

function freeze(): void {
  frozen = selection.value
}

function take(use: (q: Quote) => void): void {
  const q = frozen ?? selection.value
  frozen = null
  selection.value = null
  getSelection()?.removeAllRanges()
  if (q) use(q)
}

function quote(q: Quote): void {
  addQuote(q)
  requestComposerFocus()
}

onMounted(() => document.addEventListener('selectionchange', onSelectionChange))
onBeforeUnmount(() => {
  document.removeEventListener('selectionchange', onSelectionChange)
  clearTimeout(timer)
})
</script>

<template>
  <Transition
    enter-active-class="transition duration-200 ease-soft"
    enter-from-class="translate-y-1.5 scale-[.98] opacity-0"
    leave-active-class="transition duration-150 ease-soft"
    leave-to-class="translate-y-1 opacity-0"
  >
    <!-- pointerdown: a press anywhere on the bar keeps the selection (a
         mouse would collapse it) -->
    <div
      v-if="selection"
      class="flex max-w-full min-w-0 items-center gap-1.5 rounded-full border border-line bg-surface py-1 pr-1 pl-3.5 shadow-pop select-none"
      @pointerdown.prevent="freeze"
    >
      <span class="max-w-[22em] min-w-0 truncate text-meta text-ink-2"
        >“{{ selection.text.replace(/\s+/g, ' ') }}”</span
      >
      <button class="quote-key bg-brand text-on-brand" @click="take(quote)">
        <Icon name="quote" :size="14" />{{ t('chat.side.quote') }}
      </button>
      <button class="quote-key bg-agent text-surface" @click="take((q) => openSide(null, q))">
        <Icon name="side" :size="14" />{{ t('chat.side.title') }}
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.quote-key {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 10px;
  border-radius: 99px;
  font-size: var(--text-meta);
  font-weight: 600;
  touch-action: manipulation;
  transition: scale 0.15s var(--ease-soft);
}

.quote-key:active {
  scale: 0.95;
}
</style>
