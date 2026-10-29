<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import Sidebar from './components/Sidebar.vue'
import ChatView from './components/chat/ChatView.vue'
import RightPanel from './components/RightPanel.vue'
import Toasts from './components/Toasts.vue'
import { closePanels, layout } from './lib/layout'

// Below xl the inspector is a drawer, below lg the sidebar is one too; the
// header buttons in ChatView toggle them and the scrims below sit over the chat.
// Keep the open state honest when the viewport crosses those breakpoints.
let lg: MediaQueryList | null = null
let xl: MediaQueryList | null = null
/** Whether the column is static (not a drawer) at the current viewport. */
const lgUp = ref(false)
const xlUp = ref(false)

function onBreakpoint(): void {
  lgUp.value = lg?.matches ?? false
  xlUp.value = xl?.matches ?? false
  if (lgUp.value) layout.sidebarOpen = false
  // Above xl the inspector is a static column that can still be collapsed
  // from the header; below xl it is a drawer and starts closed.
  layout.inspectorOpen = xlUp.value
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') closePanels()
}

onMounted(() => {
  lg = window.matchMedia('(min-width: 64rem)')
  xl = window.matchMedia('(min-width: 80rem)')
  onBreakpoint()
  lg.addEventListener('change', onBreakpoint)
  xl.addEventListener('change', onBreakpoint)
  window.addEventListener('keydown', onKey)
})

onBeforeUnmount(() => {
  lg?.removeEventListener('change', onBreakpoint)
  xl?.removeEventListener('change', onBreakpoint)
  window.removeEventListener('keydown', onKey)
})
</script>

<template>
  <!-- Three columns on the canvas; the chat in the middle is the one sheet of
       paper. Below xl the inspector overlays the chat, below lg so does the
       sidebar, leaving the chat alone on screen. -->
  <div
    class="grid h-full grid-cols-1 lg:grid-cols-[248px_minmax(0,1fr)]"
    :class="
      layout.inspectorOpen ? 'xl:grid-cols-[264px_minmax(0,1fr)_348px]' : 'xl:grid-cols-[264px_minmax(0,1fr)]'
    "
  >
    <Sidebar
      class="fixed inset-y-0 left-0 z-50 w-[min(86vw,300px)] border-r border-line-soft bg-canvas shadow-pop transition-transform duration-300 ease-soft lg:static lg:z-auto lg:w-auto lg:translate-x-0 lg:border-0 lg:bg-transparent lg:shadow-none"
      :class="layout.sidebarOpen ? 'translate-x-0' : '-translate-x-full lg:translate-x-0'"
      :inert="!layout.sidebarOpen && !lgUp ? '' : undefined"
      :aria-hidden="!layout.sidebarOpen && !lgUp ? 'true' : undefined"
    />
    <ChatView />
    <RightPanel
      class="fixed inset-y-0 right-0 z-50 w-[min(92vw,340px)] border-l border-line-soft bg-canvas shadow-pop transition-transform duration-300 ease-soft xl:static xl:z-auto xl:w-auto xl:translate-x-0 xl:border-0 xl:bg-transparent xl:shadow-none"
      :class="layout.inspectorOpen ? 'translate-x-0' : 'translate-x-full xl:hidden'"
      :inert="!layout.inspectorOpen && !xlUp ? '' : undefined"
      :aria-hidden="!layout.inspectorOpen && !xlUp ? 'true' : undefined"
    />
    <Toasts />

    <!-- scrims: one per drawer, hidden once that column is static again -->
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
    <Transition
      enter-active-class="transition-opacity duration-300"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-200"
      leave-to-class="opacity-0"
    >
      <div
        v-if="layout.inspectorOpen"
        class="fixed inset-0 z-40 bg-scrim xl:hidden"
        @click="layout.inspectorOpen = false"
      />
    </Transition>
  </div>
</template>
