<script setup lang="ts">
// Landing hero (no session open): the wordmark writes itself in, then one
// line asking what to do with the assistant the new session runs as — its
// avatar and name in the line open the pick of another. The composer sits right below it, in the middle of the
// sheet — on a phone, at the page's foot.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { chooseLandingAssistant, connected, landingAssistant, state } from '../../lib/store'
import { assistantCat } from '../../lib/assistantFace'
import AssistantAvatar from '../brand/AssistantAvatar.vue'
import RumuMark from '../brand/RumuMark.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import AssistantMenu from './AssistantMenu.vue'

const assistant = computed(() => state.assistants.find((a) => a.id === landingAssistant.value) ?? null)
/** The pointer is on the assistant: its cat turns to face it. */
const near = ref(false)

function pick(id: string, close: () => void): void {
  chooseLandingAssistant(id)
  close()
}
</script>

<template>
  <div class="reveal flex flex-col items-center">
    <RumuMark class="text-ink" :height="36" draw :offline="!connected" />
    <i18n-t
      keypath="chat.welcome"
      tag="h1"
      scope="global"
      class="mt-4 flex flex-wrap items-center justify-center gap-x-1.5 gap-y-1 text-center text-display font-bold text-ink"
    >
      <template #assistant>
        <Popover :label="t('chat.newSession.assistant')" placement="bottom">
          <template #trigger="{ open, toggle }">
            <button
              type="button"
              class="inline-flex min-w-0 items-center gap-2 rounded-[12px] p-1 pr-2 transition-colors hover:bg-surface-2"
              :class="{ 'bg-surface-2': open }"
              :aria-expanded="open"
              @click="toggle"
              @pointerenter="near = true"
              @pointerleave="near = false"
            >
              <AssistantAvatar
                v-if="assistant"
                :key="assistant.id"
                :id="assistant.id"
                :cat="assistantCat(assistant)"
                :size="32"
                :near="near || open"
              />
              <span class="truncate">{{ assistant?.name ?? t('chat.newSession.pickAssistant') }}</span>
              <Icon name="down" :size="14" class="text-ink-3" />
            </button>
          </template>
          <template #default="{ close }">
            <AssistantMenu :current="landingAssistant" @pick="pick($event, close)" @close="close" />
          </template>
        </Popover>
      </template>
    </i18n-t>
  </div>
</template>
