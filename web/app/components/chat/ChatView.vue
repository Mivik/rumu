<script setup lang="ts">
// Center column, the one paper sheet on the canvas: the shell around the
// open session's view. The shell owns the header (title, diagnostics, view
// tabs, and beside them who the session runs as and the project it works in)
// and the composer card at the foot; the view between them is the
// conversation (ConversationView) or the raw trajectory (TraceView). The
// content column is capped at --col-max (main.css), centered. On phones and
// in portrait the sheet drops its card edges and becomes the page (main.css).
//
// The landing page picks the new session's assistant and project in a row of
// their own above the composer. On a phone the composer sits at the foot of
// the page, where it stays once the session opens; wider, the hero and the
// composer are centered together.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { connected, isBusy, queuedPrompts, state } from '../../lib/store'
import { layout, toggleSidebar } from '../../lib/layout'
import { openPage } from '../../lib/router'
import CardDrawer from './CardDrawer.vue'
import Composer from './Composer.vue'
import ConversationView from './ConversationView.vue'
import TasksPanel from './TasksPanel.vue'
import TraceView from './TraceView.vue'
import ViewTabs from './ViewTabs.vue'
import Welcome from './Welcome.vue'
import SessionBar from './SessionBar.vue'
import SessionPickers from './SessionPickers.vue'
import RumuCat from '../brand/RumuCat.vue'
import RumuMark from '../brand/RumuMark.vue'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'

const composerEl = ref<HTMLElement | null>(null)

/** The open session's assistant and project, by name. */
const scope = computed(() => {
  const s = state.session
  if (!s) return null
  const assistant = state.assistants.find((a) => a.id === s.assistant)
  const project = s.project ? state.projects.find((p) => p.id === s.project) : null
  return {
    assistant: s.assistant,
    assistantName: assistant?.name ?? s.assistant.slice(0, 8),
    project: s.project ?? null,
    projectName: project?.name ?? s.project ?? null,
    projectInfo: project ?? null,
  }
})

/** The view reads the composer through a getter: a template ref passed
 *  straight through a prop trips vue-tsc's HTMLElement unwrap, hence the
 *  cast. */
function composerRoot(): HTMLElement | null {
  return composerEl.value as unknown as HTMLElement | null
}
</script>

<template>
  <section
    class="sheet m-2 flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface sm:m-3 lg:mx-0 lg:my-4"
  >
    <header class="flex-none px-4 sm:px-6" :class="{ 'border-b border-line-soft': state.sessionId }">
      <div class="flex items-center gap-2" :class="state.sessionId ? 'min-h-14' : 'min-h-16'">
        <!-- the sessions sidebar is a drawer below lg -->
        <button
          class="icon-btn -ml-1.5 size-8 flex-none lg:hidden"
          :title="t('chat.header.sessions')"
          :aria-label="t('chat.header.openSessions')"
          :aria-expanded="layout.sidebarOpen"
          @click="toggleSidebar"
        >
          <Icon name="menu" :size="16" />
        </button>
        <Transition
          mode="out-in"
          enter-active-class="transition duration-300 ease-soft"
          enter-from-class="translate-y-1 opacity-0"
          leave-active-class="transition duration-150"
          leave-to-class="opacity-0"
        >
          <span
            :key="state.sessionId ?? ''"
            class="min-w-0 truncate text-title font-bold"
            :title="state.session?.title"
          >
            {{ state.session?.title || (state.sessionId ? t('common.untitled') : '') }}
          </span>
        </Transition>
        <span class="ml-auto flex min-w-0 flex-none items-center gap-2">
          <Transition
            enter-active-class="transition duration-200 ease-soft"
            enter-from-class="translate-x-1 opacity-0"
            leave-active-class="transition duration-200"
            leave-to-class="opacity-0"
          >
            <span
              v-if="isBusy"
              class="mr-1 inline-flex items-center gap-2 text-micro font-semibold text-brand-strong"
            >
              <span class="dot-live size-1.5 rounded-full bg-brand" />
              <span class="hidden sm:inline">{{ t('chat.header.running') }}</span>
            </span>
          </Transition>
          <SessionBar v-if="state.sessionId" />
        </span>
      </div>
      <div v-if="state.sessionId" class="flex gap-4">
        <ViewTabs class="flex-none" />
        <span v-if="scope" class="ml-auto flex min-w-0 items-center gap-1 pb-1.5">
          <button
            type="button"
            class="scope-tag"
            :title="t('chat.newSession.assistant')"
            @click="openPage('assistants', scope.assistant)"
          >
            <RumuCat :size="13" :paper="false" class="flex-none" />
            <span class="truncate">{{ scope.assistantName }}</span>
          </button>
          <button
            v-if="scope.project"
            type="button"
            class="scope-tag"
            :title="t('chat.sidebar.projectSettings')"
            @click="openPage('projects', scope.project)"
          >
            <ProjectSeal v-if="scope.projectInfo" :project="scope.projectInfo" :size="14" />
            <Icon v-else name="mount" :size="13" class="flex-none text-tool" />
            <span class="truncate">{{ scope.projectName }}</span>
          </button>
        </span>
      </div>
    </header>

    <Transition
      mode="out-in"
      enter-active-class="transition duration-300 ease-soft"
      enter-from-class="translate-y-2 opacity-0"
      leave-active-class="transition duration-150 ease-soft"
      leave-to-class="-translate-y-1 opacity-0"
    >
      <!-- landing: the composer is the one thing to touch -->
      <div
        v-if="!state.sessionId && !state.booting"
        key="landing"
        class="flex min-h-0 flex-1 flex-col items-center overflow-y-auto px-3 pt-1 pb-3 sm:justify-center sm:px-4 sm:pb-10"
      >
        <div class="flex w-full max-w-[var(--col-max)] flex-1 flex-col sm:flex-none">
          <Welcome class="my-auto sm:my-0" />
          <div class="mt-8 mb-2 flex min-w-0 items-center gap-1.5 px-1.5">
            <SessionPickers />
          </div>
          <Composer />
        </div>
      </div>

      <!-- booting straight into a session (a refresh on `/s/<id>`): hold the
           sheet with a quietly breathing wordmark rather than flashing the
           landing page while the tree loads -->
      <div v-else-if="!state.sessionId" key="booting" class="flex min-h-0 flex-1 items-center justify-center">
        <RumuMark :height="30" live :offline="!connected" />
      </div>

      <!-- the open session: its view + the composer at the foot of the sheet -->
      <div v-else key="chat" class="relative flex min-h-0 flex-1 flex-col">
        <!-- a view switch: the new view settles in, the old one goes at once -->
        <Transition
          enter-active-class="transition-[opacity,translate] duration-300 ease-soft"
          enter-from-class="translate-y-1.5 opacity-0"
        >
          <ConversationView v-if="state.view === 'chat'" :composer-of="composerRoot" />
          <TraceView v-else />
        </Transition>

        <!-- min-h-0: a tall decision panel shrinks into the room left above the
             sheet's foot instead of pushing the send row past its edge -->
        <div ref="composerEl" class="flex min-h-0 flex-col px-3 pt-1 pb-3 sm:px-6 sm:pb-5">
          <!-- steering prompts accepted while busy, until they are written -->
          <div
            v-if="queuedPrompts.length"
            class="mx-auto mb-1.5 flex w-full max-w-[var(--col-max)] flex-col gap-1"
          >
            <div
              v-for="q in queuedPrompts"
              :key="q.id"
              class="ml-auto w-fit max-w-[85%] rounded-sheet rounded-br-xs border border-dashed border-brand/40 bg-brand-soft/50 px-4 py-2 text-message text-ink-3 sm:max-w-[70%]"
            >
              <span class="mr-1.5 text-micro font-semibold text-ink-2">{{ t('chat.header.queued') }}</span
              >{{ q.text }}
            </div>
          </div>
          <!-- background tasks of this branch: a tab on the composer's edge -->
          <TasksPanel />
          <Composer />
        </div>
      </div>
    </Transition>

    <CardDrawer />
  </section>
</template>
