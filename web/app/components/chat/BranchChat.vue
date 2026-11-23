<script setup lang="ts">
// A branch's conversation with the composer at its foot: the chat's main
// line (ChatView), or a sub-agent beside it (AgentPanel provides its view).
// The slot replaces the conversation — the chat puts its other views there —
// and gets the composer, where the rail stops. The side questions card
// floats over the conversation (not over the chat's other views).
import { ref } from 'vue'
import { t } from '../../i18n'
import { state, useBranchView } from '../../lib/store'
import Composer from './Composer.vue'
import ConversationView from './ConversationView.vue'
import SelectionBar from './SelectionBar.vue'
import SidePop from './SidePop.vue'
import TasksPanel from './TasksPanel.vue'
import Icon from '../ui/Icon.vue'

const view = useBranchView()
const queued = view.queued

const rootEl = ref<HTMLElement | null>(null)
const rootOf = (): HTMLElement | null => rootEl.value

const composerEl = ref<HTMLElement | null>(null)
/** The composer takes queued prompts back (their keys sit above it). */
const composer = ref<InstanceType<typeof Composer> | null>(null)

/** The view reads the composer through a getter: a template ref passed
 *  straight through a prop trips vue-tsc's HTMLElement unwrap, hence the
 *  cast. */
function composerRoot(): HTMLElement | null {
  return composerEl.value as unknown as HTMLElement | null
}
</script>

<template>
  <div ref="rootEl" class="relative flex min-h-0 flex-1 flex-col">
    <slot :composer-of="composerRoot">
      <ConversationView :composer-of="composerRoot" />
    </slot>
    <SidePop v-if="view.sub || state.view === 'chat'" :root="rootOf" />

    <!-- min-h-0: a tall decision panel shrinks into the room left above the
         sheet's foot instead of pushing the send row past its edge -->
    <div ref="composerEl" class="relative flex min-h-0 flex-col px-3 pt-1 pb-3 sm:px-6 sm:pb-5">
      <!-- a selection's keys float over the conversation's foot -->
      <div
        class="pointer-events-none absolute inset-x-3 bottom-full z-10 flex justify-center pb-1 sm:inset-x-6 [&>*]:pointer-events-auto"
      >
        <SelectionBar />
      </div>
      <!-- steering prompts accepted while busy, until they are written -->
      <div v-if="queued.length" class="mx-auto mb-1.5 flex w-full max-w-[var(--col-max)] flex-col gap-1">
        <div
          v-for="q in queued"
          :key="q.id"
          class="ml-auto flex w-fit max-w-[85%] items-start gap-1 rounded-sheet rounded-br-xs border border-dashed border-brand/40 bg-brand-soft/50 py-2 pr-2 pl-4 text-message text-ink-3 sm:max-w-[70%]"
        >
          <span class="min-w-0 flex-1"
            ><span class="mr-1.5 text-micro font-semibold text-ink-2">{{ t('chat.header.queued') }}</span
            >{{ q.text }}</span
          >
          <button
            class="icon-btn size-6 flex-none"
            :title="t('chat.header.takeBack')"
            :aria-label="t('chat.header.takeBack')"
            @click="composer?.takeBack(q.id)"
          >
            <Icon name="close" :size="13" />
          </button>
        </div>
      </div>
      <!-- background tasks of this branch: a tab on the composer's edge -->
      <TasksPanel />
      <Composer ref="composer" />
    </div>
  </div>
</template>
