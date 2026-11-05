<script setup lang="ts">
// A sheet centered on a scrim, teleported to the body. Escape is handled by
// the app's closePanels; a click on the scrim asks the owner to close.
defineProps<{
  open: boolean
  /** The dialog's name, read out with the role. */
  label: string
  /** Base stacking level: the scrim takes `z`, the sheet `z + 1`. */
  z: number
  /** The centering wrapper's padding and any layout tweaks. */
  wrapperClass?: string
  /** The sheet's size and radius. */
  panelClass?: string
}>()

const emit = defineEmits<{ close: [] }>()
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-opacity duration-300"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-200"
      leave-to-class="opacity-0"
    >
      <div v-if="open" class="fixed inset-0 bg-scrim" :style="{ zIndex: z }" @click="emit('close')" />
    </Transition>
    <Transition
      enter-active-class="transition-[opacity,translate] duration-300 ease-soft"
      enter-from-class="translate-y-4 opacity-0"
      leave-active-class="transition-[opacity,translate] duration-150 ease-in"
      leave-to-class="translate-y-2 opacity-0"
    >
      <div
        v-if="open"
        class="pointer-events-none fixed inset-0 flex items-center justify-center"
        :class="wrapperClass"
        :style="{ zIndex: z + 1 }"
      >
        <section
          role="dialog"
          aria-modal="true"
          :aria-label="label"
          class="pointer-events-auto flex flex-col overflow-hidden bg-surface shadow-pop"
          :class="panelClass"
        >
          <slot />
        </section>
      </div>
    </Transition>
  </Teleport>
</template>
