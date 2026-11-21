<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, type Component } from 'vue'
import Sidebar from './components/Sidebar.vue'
import AuthGate from './components/AuthGate.vue'
import { auth } from './lib/auth'
import ChatView from './components/chat/ChatView.vue'
import AssistantsPage from './components/pages/AssistantsPage.vue'
import LibraryPage from './components/pages/LibraryPage.vue'
import ProjectsPage from './components/pages/ProjectsPage.vue'
import SettingsPage from './components/pages/SettingsPage.vue'
import Lightbox from './components/Lightbox.vue'
import DropOverlay from './components/DropOverlay.vue'
import AgentPanel from './components/chat/AgentPanel.vue'
import ArtifactPanel from './components/chat/ArtifactPanel.vue'
import TaskOutputPanel from './components/chat/TaskOutputPanel.vue'
import { openArtifact } from './lib/artifacts'
import Toasts from './components/Toasts.vue'
import StopConfirm from './components/chat/StopConfirm.vue'
import Modal from './components/ui/Modal.vue'
import { t } from './i18n'
import { closePanels, itemDialog, layout, leaveItem, WIDE } from './lib/layout'
import { SIDE_SHEET, SWAP } from './lib/motion'
import { closePage, sheetOf, type Page } from './lib/router'
import { state } from './lib/store'

/** The side sheet's place: an overlay below xl, the third column at xl. */
const SIDE =
  'fixed inset-2 z-45 shadow-pop sm:inset-3 xl:static xl:z-auto xl:col-start-3 xl:row-start-1 xl:my-4 xl:mr-4 xl:shadow-none'

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

function onBreakpoint(): void {
  layout.wide = lg?.matches ?? false
  if (layout.wide) layout.sidebarOpen = false
}

/** Settings beside the sidebar: a dialog over the desk, not a sheet. */
const settingsDialog = computed(() => layout.wide && state.page?.page === 'settings')
/** The page on the chat's sheet, if any. */
const sheetPage = computed(() => (settingsDialog.value ? null : state.page))

function onKey(e: KeyboardEvent): void {
  if (e.key !== 'Escape') return
  if (state.drawers.length) return
  if (settingsDialog.value) closePage()
  else if (itemDialog()) leaveItem()
  else closePanels()
}

/** An artifact is open beside the chat (the session is showing). */
const artifactOpen = computed(() => !sheetPage.value && !!state.sessionId && !!openArtifact.value)
/** A sub-agent is open beside the chat instead. */
const agentOpen = computed(
  () => !sheetPage.value && !artifactOpen.value && !!state.agentView && !!state.branches[state.agentView],
)
/** Or a background task's output. */
const taskOpen = computed(
  () =>
    !sheetPage.value &&
    !artifactOpen.value &&
    !agentOpen.value &&
    !!state.taskDetail &&
    !!state.tasks[state.taskDetail],
)

onMounted(() => {
  lg = window.matchMedia(WIDE)
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
  <AuthGate v-if="auth.status !== 'ready'" />
  <!-- Two sheets of paper on the canvas (three while an artifact or a
       sub-agent or a task's output is open at xl): the sessions (a card at lg, the
       drawer below it) and the chat — the one the conversation is written on.
       Everything about the open session (branches, cards in play, diagnostics)
       lives in that sheet's header; below lg the sidebar overlays the chat,
       leaving it alone on screen. A management page (the library, the
       workspaces, …) takes the chat's place while its route is open; the chat
       stays mounted beneath it, so its draft and scroll survive. The cards
       share one 16px gutter on every side, so the desk reads as an even
       grid. -->
  <!-- At xl the third column is always there, at 0fr while nothing is open in
       it: the column eases open and shut (fr tracks interpolate) instead of
       snapping the chat to half its width. A track's width already eases out
       as its fr grows, hence ease-in-out. The third sheet keeps its own right
       gutter, so the desk drops its padding there, and clips the sheet while
       it slides past the edge (motion.ts, SIDE_SHEET). -->
  <div
    v-else
    class="desk grid h-full grid-cols-1 transition-[grid-template-columns] duration-350 ease-in-out lg:grid-cols-[280px_minmax(0,1fr)] lg:gap-x-4 lg:pr-4 xl:overflow-x-clip xl:pr-0"
    :class="
      artifactOpen || agentOpen || taskOpen
        ? 'xl:grid-cols-[280px_minmax(0,1fr)_minmax(0,1fr)]'
        : 'xl:grid-cols-[280px_minmax(0,1fr)_minmax(0,0fr)]'
    "
  >
    <Sidebar
      class="fixed inset-y-0 left-0 z-50 w-[min(86vw,300px)] border-r border-line-soft bg-canvas transition-transform duration-300 ease-soft lg:static lg:z-auto lg:my-4 lg:ml-4 lg:w-auto lg:translate-x-0 lg:rounded-sheet lg:border-0 lg:bg-surface lg:shadow-card lg:ring-1 lg:ring-line"
      :class="layout.sidebarOpen ? 'translate-x-0 shadow-pop' : '-translate-x-full lg:translate-x-0'"
      :inert="!layout.sidebarOpen && !layout.wide ? '' : undefined"
      :aria-hidden="!layout.sidebarOpen && !layout.wide ? 'true' : undefined"
    />
    <!-- the chat and the pages share one cell, so a switch cross-fades -->
    <Transition v-bind="SWAP">
      <ChatView v-show="!sheetPage" class="col-start-1 row-start-1 lg:col-start-2" />
    </Transition>
    <Transition v-bind="SWAP">
      <component
        :is="PAGES[sheetPage.page]"
        v-if="sheetPage"
        :key="sheetOf(sheetPage.page)"
        class="col-start-1 row-start-1 lg:col-start-2"
      />
    </Transition>
    <!-- the third sheet at xl, over the chat below it: an open artifact, a
         sub-agent, or a background task's output -->
    <Transition v-bind="SIDE_SHEET">
      <ArtifactPanel v-if="artifactOpen" key="artifact" :class="SIDE" />
      <AgentPanel v-else-if="agentOpen" key="agent" :class="SIDE" />
      <TaskOutputPanel v-else-if="taskOpen" key="task" :class="SIDE" />
    </Transition>
    <!-- settings at lg: a dialog over the desk, which stays as it was -->
    <Modal
      :open="settingsDialog"
      :label="t('settings.title')"
      :z="55"
      wrapper-class="p-8"
      panel-class="h-[min(48rem,100%)] w-[min(64rem,100%)] rounded-sheet"
      @close="closePage"
    >
      <SettingsPage modal />
    </Modal>
    <Lightbox />
    <StopConfirm />
    <Toasts />
    <!-- a file dropped anywhere in the app lands in the composer -->
    <DropOverlay />

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
