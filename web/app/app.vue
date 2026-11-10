<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, type Component } from 'vue'
import Sidebar from './components/Sidebar.vue'
import ChatView from './components/chat/ChatView.vue'
import AssistantsPage from './components/pages/AssistantsPage.vue'
import LibraryPage from './components/pages/LibraryPage.vue'
import ProjectsPage from './components/pages/ProjectsPage.vue'
import SettingsPage from './components/pages/SettingsPage.vue'
import Lightbox from './components/Lightbox.vue'
import AgentPanel from './components/chat/AgentPanel.vue'
import ArtifactPanel from './components/chat/ArtifactPanel.vue'
import { openArtifact } from './lib/artifacts'
import Toasts from './components/Toasts.vue'
import { closePanels, layout } from './lib/layout'
import type { Page } from './lib/router'
import { state } from './lib/store'

const PAGES: Record<Page, Component> = {
  decks: LibraryPage,
  workspaces: ProjectsPage,
  assistants: AssistantsPage,
  projects: ProjectsPage,
  settings: SettingsPage,
}

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

/** An artifact is open beside the chat (the session is showing). */
const artifactOpen = computed(() => !state.page && !!state.sessionId && !!openArtifact.value)
/** A sub-agent's transcript is open beside the chat instead. */
const agentOpen = computed(
  () => !state.page && !artifactOpen.value && !!state.agentView && !!state.branches[state.agentView],
)

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
  <!-- Two sheets of paper on the canvas (three while an artifact or a
       sub-agent is open at xl): the sessions (a card at lg, the
       drawer below it) and the chat — the one the conversation is written on.
       Everything about the open session (branches, cards in play, diagnostics)
       lives in that sheet's header; below lg the sidebar overlays the chat,
       leaving it alone on screen. A management page (the library, the
       workspaces, …) takes the chat's place while its route is open; the chat
       stays mounted beneath it, so its draft and scroll survive. The cards
       share one 16px gutter on every side, so the desk reads as an even
       grid. -->
  <div
    class="desk grid h-full grid-cols-1 lg:grid-cols-[280px_minmax(0,1fr)] lg:gap-x-4 lg:pr-4"
    :class="{ 'xl:grid-cols-[280px_minmax(0,1fr)_minmax(0,1fr)]': artifactOpen || agentOpen }"
  >
    <Sidebar
      class="fixed inset-y-0 left-0 z-50 w-[min(86vw,300px)] border-r border-line-soft bg-canvas transition-transform duration-300 ease-soft lg:static lg:z-auto lg:my-4 lg:ml-4 lg:w-auto lg:translate-x-0 lg:rounded-sheet lg:border-0 lg:bg-surface lg:shadow-card lg:ring-1 lg:ring-line"
      :class="layout.sidebarOpen ? 'translate-x-0 shadow-pop' : '-translate-x-full lg:translate-x-0'"
      :inert="!layout.sidebarOpen && !lgUp ? '' : undefined"
      :aria-hidden="!layout.sidebarOpen && !lgUp ? 'true' : undefined"
    />
    <ChatView v-show="!state.page" />
    <component :is="PAGES[state.page.page]" v-if="state.page" />
    <!-- an open artifact: the third sheet at xl, over the chat below it -->
    <ArtifactPanel
      v-if="artifactOpen"
      class="fixed inset-2 z-45 shadow-pop sm:inset-3 xl:static xl:z-auto xl:my-4 xl:shadow-none"
    />
    <!-- a sub-agent's transcript: the same third sheet -->
    <AgentPanel
      v-if="agentOpen"
      class="fixed inset-2 z-45 shadow-pop sm:inset-3 xl:static xl:z-auto xl:my-4 xl:shadow-none"
    />
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
