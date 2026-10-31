<script setup lang="ts">
// The image lightbox: one overlay mounted at the app root, opened from any
// thumbnail (`openLightbox`). Escape or a click on the scrim closes it. The
// scrim is deliberately darker than `--scrim`: a photograph needs the desk
// to step back.
import { onBeforeUnmount, onMounted } from 'vue'
import { t } from '../i18n'
import { closeLightbox, state } from '../lib/store'
import Icon from './ui/Icon.vue'

// Capture phase, before the panels and drawers below it hear the key.
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && state.lightbox) {
    e.stopPropagation()
    closeLightbox()
  }
}

onMounted(() => window.addEventListener('keydown', onKey, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-opacity duration-200"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-150"
      leave-to-class="opacity-0"
    >
      <div
        v-if="state.lightbox"
        class="fixed inset-0 z-70 flex flex-col items-center justify-center gap-3 p-6"
        style="background: rgb(24 19 15 / 85%)"
        role="dialog"
        aria-modal="true"
        :aria-label="state.lightbox.name ?? t('chat.image.open')"
        @click="closeLightbox()"
      >
        <img
          :src="state.lightbox.src"
          :alt="state.lightbox.name ?? t('chat.image.alt')"
          class="max-h-[86vh] max-w-[92vw] rounded-control shadow-pop"
          @click.stop
        />
        <div v-if="state.lightbox.name" class="flex max-w-[80vw] items-baseline gap-2">
          <span class="truncate text-meta text-white/80">{{ state.lightbox.name }}</span>
          <span v-if="state.lightbox.width" class="flex-none font-mono text-micro text-white/50">
            {{ state.lightbox.width }}×{{ state.lightbox.height }}
          </span>
        </div>
        <button
          class="absolute top-4 right-4 flex size-9 items-center justify-center rounded-full text-white/80 transition-colors hover:bg-white/10 hover:text-white"
          :title="t('chat.image.close')"
          :aria-label="t('chat.image.close')"
          @click="closeLightbox()"
        >
          <Icon name="close" :size="16" />
        </button>
      </div>
    </Transition>
  </Teleport>
</template>
