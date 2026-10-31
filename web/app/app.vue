<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import Sidebar from './components/Sidebar.vue'
import ChatView from './components/chat/ChatView.vue'
import CardCatalog from './components/chat/CardCatalog.vue'
import Lightbox from './components/Lightbox.vue'
import Toasts from './components/Toasts.vue'
import { closePanels, layout } from './lib/layout'

// Below lg the sidebar is a drawer; the header button in ChatView toggles it
// and the scrim below sits over the chat. Keep the open state honest when the
// viewport crosses that breakpoint.
let lg: MediaQueryList | null = null
/** Whether the sidebar is static (not a drawer) at the current viewport. */
const lgUp = ref(false)

function onBreakpoint(): void {
  lgUp.value = lg?.matches ?? false
  if (lgUp.value) layout.sidebarOpen = false
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') closePanels()
}

onMounted(() => {
  lg = window.matchMedia('(min-width: 64rem)')
  onBreakpoint()
  lg.addEventListener('change', onBreakpoint)
  window.addEventListener('keydown', onKey)
})

onBeforeUnmount(() => {
  lg?.removeEventListener('change', onBreakpoint)
  window.removeEventListener('keydown', onKey)
})
</script>

<template>
  <!-- Two sheets of paper on the canvas: the sessions (a card at lg, the
       drawer below it) and the chat — the one the conversation is written on.
       Everything about the open session (branches, cards in play, diagnostics)
       lives in that sheet's header; below lg the sidebar overlays the chat,
       leaving it alone on screen. The cards share one 16px gutter on every
       side, so the desk reads as an even grid. -->
  <div class="desk grid h-full grid-cols-1 lg:grid-cols-[280px_minmax(0,1fr)] lg:gap-x-4 lg:pr-4">
    <Sidebar
      class="fixed inset-y-0 left-0 z-50 w-[min(86vw,300px)] border-r border-line-soft bg-canvas transition-transform duration-300 ease-soft lg:static lg:z-auto lg:my-4 lg:ml-4 lg:w-auto lg:translate-x-0 lg:rounded-sheet lg:border-0 lg:bg-surface lg:shadow-card lg:ring-1 lg:ring-line"
      :class="layout.sidebarOpen ? 'translate-x-0 shadow-pop' : '-translate-x-full lg:translate-x-0'"
      :inert="!layout.sidebarOpen && !lgUp ? '' : undefined"
      :aria-hidden="!layout.sidebarOpen && !lgUp ? 'true' : undefined"
    />
    <ChatView />
    <CardCatalog />
    <Lightbox />
    <Toasts />

    <Transition
      enter-active-class="transition-opacity duration-300"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-200"
      leave-to-class="opacity-0"
    >
      <div
        v-if="layout.sidebarOpen"
        class="fixed inset-0 z-40 bg-scrim lg:hidden"
        @click="layout.sidebarOpen = false"
      />
    </Transition>
  </div>
</template>
