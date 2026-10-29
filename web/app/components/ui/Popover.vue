<script setup lang="ts">
// A click-open popover: a trigger (slot `trigger`, given `open` + `toggle`)
// and a paper panel floated beside it (default slot, given `close`).
// Placement is @floating-ui/vue (fixed strategy, flip + shift), teleported to
// <body> so no scroller or `overflow: hidden` sheet can clip it; it stays
// hidden until placed, so it never flashes at 0,0. A pointer down outside
// both, or Escape, closes it.
import { onBeforeUnmount, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/vue'
import type { Placement } from '@floating-ui/vue'

const props = withDefaults(defineProps<{ placement?: Placement; label: string; wide?: boolean }>(), {
  placement: 'bottom-end',
  wide: false,
})

const open = ref(false)
const anchor = ref<HTMLElement | null>(null)
const panel = ref<HTMLElement | null>(null)

function toggle(): void {
  open.value = !open.value
}
function close(): void {
  open.value = false
}

const { floatingStyles, isPositioned } = useFloating(
  // Template refs are narrowed by vue-tsc; floating-ui just reads them.
  anchor as unknown as Readonly<Ref<HTMLElement | null>>,
  panel as unknown as Readonly<Ref<HTMLElement | null>>,
  {
    open,
    placement: props.placement,
    strategy: 'fixed',
    // left/top rather than a transform: the entrance below animates
    // `translate`, and must never carry the panel in from the corner.
    transform: false,
    whileElementsMounted: autoUpdate,
    middleware: [offset(8), flip({ padding: 8 }), shift({ padding: 8 })],
  },
)

function onPointer(e: PointerEvent): void {
  const target = e.target as Node
  if (!anchor.value?.contains(target) && !panel.value?.contains(target)) close()
}
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') close()
}

watch(open, (v) => {
  if (v) {
    document.addEventListener('pointerdown', onPointer, true)
    window.addEventListener('keydown', onKey)
  } else {
    document.removeEventListener('pointerdown', onPointer, true)
    window.removeEventListener('keydown', onKey)
  }
})

onBeforeUnmount(close)
</script>

<template>
  <div ref="anchor" class="inline-flex">
    <slot name="trigger" :open="open" :toggle="toggle" />
  </div>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-[opacity,translate] duration-150 ease-soft"
      enter-from-class="-translate-y-1 opacity-0"
      leave-active-class="transition-opacity duration-100"
      leave-to-class="opacity-0"
    >
      <div
        v-if="open"
        ref="panel"
        role="dialog"
        :aria-label="label"
        :style="{ ...floatingStyles, visibility: isPositioned ? 'visible' : 'hidden' }"
        class="z-50 max-h-[min(70vh,560px)] overflow-y-auto rounded-card border border-line-soft bg-surface p-3 shadow-pop"
        :class="wide ? 'w-[min(440px,calc(100vw-1rem))]' : 'w-[min(360px,calc(100vw-1rem))]'"
      >
        <slot :close="close" />
      </div>
    </Transition>
  </Teleport>
</template>
