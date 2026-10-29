<script setup lang="ts">
// The message composer. It lives at the bottom of the chat, and at the center
// of the landing state — in which case sending the first message creates the
// session (store.sendPrompt does that).
//
// A sheet of paper with the writing on top and a footer that says where the
// prompt will go: the branch's default workspace (where its commands run —
// it opens the list of all workspaces) and the model answering it, on the
// left; the send button on the right —
// solid coral once there is something to send. While rumu works, sending
// steers the turn and a stop button joins it.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { isBusy, pathNodes, sendPrompt, state, stopTurn } from '../../lib/store'
import { INTERNAL_WORKSPACE } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import WorkspacesPanel from '../WorkspacesPanel.vue'

const draft = ref('')
const textareaEl = ref<HTMLTextAreaElement>()
const canSend = computed(() => draft.value.trim() !== '')

async function submit(): Promise<void> {
  const text = draft.value.trim()
  if (!text) return
  draft.value = ''
  // Put the text back if creating/sending failed so nothing typed is lost.
  if (!(await sendPrompt(text))) draft.value = text
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
    e.preventDefault()
    submit()
  }
}

const placeholder = computed(() => {
  return isBusy.value ? t('chat.composer.steerPlaceholder') : t('chat.composer.placeholder')
})

/** The default workspace of the open branch, with where it lives on hover. */
const workspace = computed(() => {
  const ps = state.pathState
  const name = ps?.default_workspace
  if (!ps || !name) return null
  if (name === INTERNAL_WORKSPACE) return { name, title: t('panels.context.virtualRoot') }
  const ws = ps.workspaces.find((w) => w.name === name)
  return { name, title: ws ? `${ws.ssh ? `ssh:${ws.ssh}` : 'local'} ${ws.root}` : name }
})

/** The model that answered last on this path. */
const model = computed(() => {
  for (let i = pathNodes.value.length - 1; i >= 0; i--) {
    const n = pathNodes.value[i]
    if (n.body.type === 'assistant') return n.body.model
  }
  return null
})
</script>

<template>
  <!-- the footer's empty stretch is part of the page: clicking it writes -->
  <div
    class="composer mx-auto flex w-full max-w-[760px] flex-col rounded-sheet"
    @click.self="textareaEl?.focus()"
  >
    <textarea
      ref="textareaEl"
      v-model="draft"
      class="max-h-[240px] min-h-[52px] resize-none rounded-none border-0 bg-transparent px-5 pt-4 pb-1 text-message leading-normal [field-sizing:content] focus:shadow-none"
      rows="2"
      :placeholder="placeholder"
      @keydown="onKeydown"
    />
    <div class="flex min-w-0 items-center gap-2 pt-1 pr-3 pb-3 pl-4" @click.self="textareaEl?.focus()">
      <Popover v-if="workspace" :label="t('panels.context.workspaces')" placement="top-start">
        <template #trigger="{ open, toggle }">
          <button
            type="button"
            class="inline-flex min-w-0 items-center gap-1.5 rounded-full bg-surface-2 py-1 pr-2 pl-2.5 text-meta font-semibold text-ink-2 transition-colors hover:text-ink"
            :class="{ 'text-ink': open }"
            :title="workspace.title"
            :aria-expanded="open"
            @click="toggle"
          >
            <Icon name="mount" :size="13" class="text-tool" />
            <span class="truncate">{{ workspace.name }}</span>
            <Icon name="down" :size="11" class="text-ink-3" />
          </button>
        </template>
        <div class="panel-head">{{ t('panels.context.workspaces') }}</div>
        <WorkspacesPanel />
      </Popover>
      <span v-if="model" class="hidden min-w-0 truncate text-meta text-ink-3 sm:inline" :title="model">
        {{ model }}
      </span>
      <span class="flex-1" @click.self="textareaEl?.focus()" />
      <Transition
        enter-active-class="transition duration-200 ease-spring"
        enter-from-class="scale-75 opacity-0"
        leave-active-class="transition duration-100"
        leave-to-class="scale-75 opacity-0"
      >
        <button
          v-if="isBusy"
          class="composer-stop"
          :title="t('chat.composer.stop')"
          :aria-label="t('chat.composer.stop')"
          @click="stopTurn()"
        >
          <Icon name="stop" :size="18" />
        </button>
      </Transition>
      <button
        class="composer-send"
        :disabled="!canSend"
        :title="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        :aria-label="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        @click="submit"
      >
        <Icon name="send" :size="18" :stroke="2.4" />
      </button>
    </div>
  </div>
</template>
