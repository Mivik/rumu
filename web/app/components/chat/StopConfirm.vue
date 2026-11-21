<script setup lang="ts">
// Asked when moving the main line (regenerate, edit, variant switch, rewind)
// would leave background tasks or sub-agents of the part it leaves behind:
// a small popup beside the control the user pressed, the session menu's
// confirm step — stop them and go on, or keep everything as it is. A pointer
// down outside it, or Escape, says no; the page does not scroll meanwhile.
import { computed, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/vue'
import { t } from '../../i18n'
import { stopConfirm } from '../../lib/state/ui'
import ConfirmPrompt from '../ui/ConfirmPrompt.vue'

const panel = ref<HTMLElement | null>(null)
const open = computed(() => !!stopConfirm.value)

// The control may leave the page meanwhile (a hover-revealed row): keep its
// last box; with no control at all, the middle of the screen.
let lastRect: DOMRect | null = null
const reference = computed(() => {
  const anchor = stopConfirm.value?.anchor ?? null
  lastRect = null
  return {
    getBoundingClientRect(): DOMRect {
      if (anchor?.isConnected) lastRect = anchor.getBoundingClientRect()
      return lastRect ?? new DOMRect(innerWidth / 2, innerHeight / 2, 0, 0)
    },
  }
})

const { floatingStyles, isPositioned } = useFloating(
  reference,
  panel as unknown as Readonly<Ref<HTMLElement | null>>,
  {
    open,
    placement: 'top',
    strategy: 'fixed',
    transform: false,
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip({ padding: 8 }), shift({ padding: 8 })],
  },
)

const answer = (ok: boolean): void => stopConfirm.value?.answer(ok)

function onPointer(e: PointerEvent): void {
  if (!panel.value?.contains(e.target as Node)) answer(false)
}
/** Wheel, touch drag or a scrolling key outside the popup moves nothing. */
function onScroll(e: Event): void {
  if (!panel.value?.contains(e.target as Node)) e.preventDefault()
}
const SCROLL_KEYS = new Set(['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '])

function onKey(e: KeyboardEvent): void {
  if (SCROLL_KEYS.has(e.key) && !panel.value?.contains(e.target as Node)) e.preventDefault()
  if (e.key !== 'Escape') return
  // Close only the popup: a dialog behind it stays open.
  e.preventDefault()
  e.stopPropagation()
  answer(false)
}

watch(open, (v) => {
  if (v) {
    document.addEventListener('pointerdown', onPointer, true)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('wheel', onScroll, { capture: true, passive: false })
    window.addEventListener('touchmove', onScroll, { capture: true, passive: false })
  } else {
    document.removeEventListener('pointerdown', onPointer, true)
    window.removeEventListener('keydown', onKey, true)
    window.removeEventListener('wheel', onScroll, true)
    window.removeEventListener('touchmove', onScroll, true)
  }
})
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-[opacity,translate] duration-150 ease-soft"
      enter-from-class="translate-y-1 opacity-0"
      leave-active-class="transition-opacity duration-100"
      leave-to-class="opacity-0"
    >
      <div
        v-if="stopConfirm"
        ref="panel"
        role="alertdialog"
        :aria-label="t('chat.stopConfirm.body', { count: stopConfirm.count })"
        :style="{ ...floatingStyles, visibility: isPositioned ? 'visible' : 'hidden' }"
        class="z-70 w-[min(220px,calc(100vw-1rem))] rounded-card border border-line-soft bg-surface p-1 shadow-pop"
      >
        <ConfirmPrompt
          :prompt="t('chat.stopConfirm.body', { count: stopConfirm.count })"
          :confirm="t('chat.stopConfirm.submit')"
          @cancel="answer(false)"
          @confirm="answer(true)"
        />
      </div>
    </Transition>
  </Teleport>
</template>
