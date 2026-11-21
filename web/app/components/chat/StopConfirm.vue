<script setup lang="ts">
// Asked when moving the main line (regenerate, edit, variant switch, rewind)
// would leave background tasks or sub-agents of the part it leaves behind:
// a small popup beside the control the user pressed — stop them and go on,
// or keep everything as it is. A pointer down outside it, or Escape, says no.
import { computed, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/vue'
import { t } from '../../i18n'
import { stopConfirm } from '../../lib/state/ui'

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
function onKey(e: KeyboardEvent): void {
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
  } else {
    document.removeEventListener('pointerdown', onPointer, true)
    window.removeEventListener('keydown', onKey, true)
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
        class="z-70 flex w-max max-w-[min(320px,calc(100vw-1rem))] flex-col gap-2 rounded-card border border-line-soft bg-surface p-3 shadow-pop"
      >
        <p class="text-secondary text-ink-2">
          {{ t('chat.stopConfirm.body', { count: stopConfirm.count }) }}
        </p>
        <div class="flex items-center justify-end gap-2">
          <button type="button" class="btn" @click="answer(false)">{{ t('common.cancel') }}</button>
          <button type="button" class="btn btn-primary" autofocus @click="answer(true)">
            {{ t('chat.stopConfirm.submit') }}
          </button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
