<script setup lang="ts">
// A click-open popover: a trigger (slot `trigger`, given `open` + `toggle`)
// and a paper panel floated beside it (default slot, given `close`).
// Placement is @floating-ui/vue (fixed strategy, flip + shift, its height
// held to the room on its side), teleported to
// <body> so no scroller or `overflow: hidden` sheet can clip it; it stays
// hidden until placed, so it never flashes at 0,0. A pointer down outside
// both, or Escape, closes it. Once placed, an `[autofocus]` inside takes focus.
import { onBeforeUnmount, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { autoUpdate, flip, offset, shift, size, useFloating } from '@floating-ui/vue'
import type { Placement } from '@floating-ui/vue'

const props = withDefaults(
  defineProps<{
    placement?: Placement
    label: string
    wide?: boolean
    /** A compact action menu (tight padding, narrow panel). */
    menu?: boolean
    /** The panel's ARIA role; `menu` for an action menu. */
    role?: 'dialog' | 'menu'
    /** Teleport target; position absolutely inside it. A modal `<dialog>`
     *  makes everything outside inert, so pass the dialog itself. */
    to?: string
  }>(),
  {
    placement: 'bottom-end',
    wide: false,
    menu: false,
    role: 'dialog',
  },
)

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
    // A contained panel is absolute to its target; the default escapes
    // scrollers and sheets as a viewport-fixed layer.
    strategy: props.to ? 'absolute' : 'fixed',
    // left/top rather than a transform: the entrance below animates
    // `translate`, and must never carry the panel in from the corner.
    transform: false,
    whileElementsMounted: autoUpdate,
    middleware: [
      offset(8),
      flip({ padding: 8 }),
      shift({ padding: 8 }),
      size({
        padding: 8,
        apply({ availableHeight, elements }) {
          elements.floating.style.maxHeight = `${Math.min(availableHeight, 560)}px`
        },
      }),
    ],
  },
)

function onPointer(e: PointerEvent): void {
  const target = e.target as Node
  if (!anchor.value?.contains(target) && !panel.value?.contains(target)) close()
}
function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Escape') return
  // Close only the popover: a dialog behind it stays open.
  e.preventDefault()
  e.stopPropagation()
  close()
  // Put focus back on the trigger that opened the panel.
  anchor.value?.querySelector<HTMLElement>('button, [href], [tabindex]')?.focus()
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

// The panel is hidden until placed, and a hidden element can't take focus:
// wait for the render that reveals it.
watch(
  isPositioned,
  (v) => {
    if (v) panel.value?.querySelector<HTMLElement>('[autofocus]')?.focus()
  },
  { flush: 'post' },
)

onBeforeUnmount(close)
</script>

<template>
  <div ref="anchor" class="inline-flex">
    <slot name="trigger" :open="open" :toggle="toggle" />
  </div>
  <Teleport :to="to || 'body'">
    <Transition
      enter-active-class="transition-[opacity,translate] duration-150 ease-soft"
      enter-from-class="-translate-y-1 opacity-0"
      leave-active-class="transition-opacity duration-100"
      leave-to-class="opacity-0"
    >
      <div
        v-if="open"
        ref="panel"
        :role="role"
        :aria-label="label"
        :style="{ ...floatingStyles, visibility: isPositioned ? 'visible' : 'hidden' }"
        class="z-60 overflow-y-auto rounded-card border border-line-soft bg-surface shadow-pop"
        :class="
          menu
            ? 'w-[min(220px,calc(100vw-1rem))] p-1'
            : wide
              ? 'w-[min(440px,calc(100vw-1rem))] p-3'
              : 'w-[min(360px,calc(100vw-1rem))] p-3'
        "
      >
        <slot :close="close" />
      </div>
    </Transition>
  </Teleport>
</template>
