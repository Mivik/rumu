<script setup lang="ts">
// Above the composer's writing: the passages quoted into the message, and the
// one selected in the conversation right now with its two uses — quote it
// here, or ask about it on the side (SidePop). Web pages cannot add to the
// browser's own selection menu, so the choice waits down here, clear of the
// phone's selection toolbar and handles.
import { onBeforeUnmount, onMounted } from 'vue'
import { t } from '../../i18n'
import type { Quote } from '../../lib/types'
import { openSide, quotes, selection } from '../../lib/state/side'
import Icon from '../ui/Icon.vue'

const emit = defineEmits<{ quoted: [] }>()

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
// on a key below collapses it before the click lands: the reading waits a
// beat, and a press freezes what it saw.
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
  quotes.value.push(q)
  emit('quoted')
}

onMounted(() => document.addEventListener('selectionchange', onSelectionChange))
onBeforeUnmount(() => {
  document.removeEventListener('selectionchange', onSelectionChange)
  clearTimeout(timer)
})
</script>

<template>
  <div v-if="quotes.length || selection" class="flex flex-col gap-1.5 px-3 pt-2.5">
    <div v-for="(q, i) in quotes" :key="i" class="flex min-w-0 items-start gap-2 pl-1">
      <p class="line-clamp-2 min-w-0 flex-1 border-l-2 border-brand pl-2.5 text-meta text-ink-2">
        {{ q.text }}
      </p>
      <button
        class="icon-btn size-6 flex-none"
        :title="t('chat.side.unquote')"
        :aria-label="t('chat.side.unquote')"
        @click="quotes.splice(i, 1)"
      >
        <Icon name="close" :size="13" />
      </button>
    </div>
    <div
      v-if="selection"
      v-enter="'enter-fade'"
      class="flex min-w-0 items-center gap-1.5 rounded-card border border-dashed border-line bg-surface-2 py-1 pr-1 pl-3"
    >
      <span class="min-w-0 flex-1 truncate text-meta text-ink-2"
        >“{{ selection.text.replace(/\s+/g, ' ') }}”</span
      >
      <!-- pointerdown: kept from the sheet's unfold, and from collapsing the
           selection on a mouse -->
      <button
        class="quote-key bg-brand text-on-brand"
        @pointerdown.stop.prevent="freeze"
        @click="take(quote)"
      >
        <Icon name="quote" :size="14" />{{ t('chat.side.quote') }}
      </button>
      <button
        class="quote-key bg-agent text-surface"
        @pointerdown.stop.prevent="freeze"
        @click="take((q) => openSide(null, q))"
      >
        <Icon name="side" :size="14" />{{ t('chat.side.title') }}
      </button>
    </div>
  </div>
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
  user-select: none;
  transition: scale 0.15s var(--ease-soft);
}

.quote-key:active {
  scale: 0.95;
}
</style>
