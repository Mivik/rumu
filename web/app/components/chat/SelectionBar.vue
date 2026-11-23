<script setup lang="ts">
// The passage selected in the conversation right now, with its two uses —
// quote it into the message, or ask about it on the side (SidePop). Web
// pages cannot add to the browser's own selection menu, so the choice floats
// just above the composer, clear of the phone's selection toolbar and
// handles. It floats over the conversation rather than taking room: a page
// followed to its end would otherwise ride up under the selection.
//
// Each conversation shown has one (BranchChat): the main line's answers a
// selection outside the sub-agent panel, the panel's one inside it.
import { onBeforeUnmount, onMounted } from 'vue'
import { t } from '../../i18n'
import type { Quote } from '../../lib/types'
import { useBranchView } from '../../lib/state/branchView'
import { addQuote, openSide } from '../../lib/state/side'
import Icon from '../ui/Icon.vue'

const view = useBranchView()
const aside = view.aside
const selection = aside.selection

/** The selection, when it lies inside one reply of this conversation. */
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
  if (!!at.closest('[data-agent-panel]') !== view.sub) return null
  return { node, text }
}

// A phone's selection settles only once its handles are let go, and a press
// on a key collapses it before the click lands: the reading waits a beat,
// and a press freezes what it saw — only while it lasts: a press that ends
// without a key's click (on the gap, dragged off, a touch turned scroll)
// lets the bar follow the selection again.
let timer = 0
let frozen: Quote | null = null
let press: AbortController | null = null

function onSelectionChange(): void {
  clearTimeout(timer)
  timer = window.setTimeout(() => {
    const q = read()
    if (q || !press) selection.value = q
    if (!press) frozen = null
  }, 200)
}

function freeze(): void {
  frozen = selection.value
  press?.abort()
  press = new AbortController()
  const release = (): void => {
    press?.abort()
    press = null
    onSelectionChange()
  }
  const opts = { capture: true, signal: press.signal }
  document.addEventListener('pointerup', release, opts)
  document.addEventListener('pointercancel', release, opts)
}

function take(use: (q: Quote) => void): void {
  const q = frozen ?? selection.value
  frozen = null
  selection.value = null
  getSelection()?.removeAllRanges()
  if (q) use(q)
}

function quote(q: Quote): void {
  addQuote(aside, q)
}

onMounted(() => document.addEventListener('selectionchange', onSelectionChange))
onBeforeUnmount(() => {
  document.removeEventListener('selectionchange', onSelectionChange)
  press?.abort()
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
      class="flex items-center gap-0.5 rounded-full border border-line bg-surface p-1 shadow-pop select-none"
      @pointerdown.prevent="freeze"
    >
      <button class="quote-key text-brand-strong hover:bg-brand-soft" @click="take(quote)">
        <Icon name="quote" :size="15" />{{ t('chat.side.quote') }}
      </button>
      <span class="h-4 w-px flex-none bg-line" />
      <button class="quote-key text-agent hover:bg-agent-soft" @click="take((q) => openSide(aside, null, q))">
        <Icon name="side" :size="15" />{{ t('chat.side.title') }}
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.quote-key {
  display: inline-flex;
  flex: none;
  align-items: center;
  gap: 5px;
  height: 30px;
  padding: 0 12px 0 10px;
  border-radius: 99px;
  font-size: var(--text-meta);
  font-weight: 600;
  touch-action: manipulation;
  transition:
    background-color 0.15s,
    scale 0.15s var(--ease-soft);
}

.quote-key:active {
  scale: 0.95;
}
</style>
