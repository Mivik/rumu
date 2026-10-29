<script setup lang="ts">
// The message composer. It lives at the bottom of the chat, and at the center
// of the landing state — in which case sending the first message creates the
// session (store.sendPrompt does that).
//
// A sheet of paper with the writing on top and a quiet footer: the key hints
// on the left, the coral dot (send) on the right. While rumu works the
// composer doesn't animate — the mascot and the header's dot already say so;
// sending then steers the turn, and a stop button joins the dot.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { isBusy, sendPrompt, stopTurn } from '../../lib/store'
import Icon from '../ui/Icon.vue'

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
</script>

<template>
  <!-- the footer's empty stretch is part of the page: clicking it writes -->
  <div
    class="composer mx-auto flex w-full max-w-[760px] flex-col rounded-card"
    @click.self="textareaEl?.focus()"
  >
    <textarea
      ref="textareaEl"
      v-model="draft"
      class="max-h-[200px] min-h-[44px] resize-none rounded-none border-0 bg-transparent px-4 pt-3 pb-0.5 text-message leading-normal [field-sizing:content] focus:shadow-none sm:px-5"
      rows="2"
      :placeholder="placeholder"
      @keydown="onKeydown"
    />
    <div
      class="flex items-center justify-end gap-1.5 pr-2 pb-2 pl-4 sm:pl-5"
      @click.self="textareaEl?.focus()"
    >
      <div class="composer-hint mr-auto hidden gap-1 text-micro text-ink-3 sm:flex" aria-hidden="true">
        <kbd class="font-sans font-semibold text-ink-2">Enter</kbd>
        {{ t('chat.composer.enterToSend') }} ·
        <kbd class="font-sans font-semibold text-ink-2">Shift + Enter</kbd>
        {{ t('chat.composer.shiftEnter') }}
      </div>
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
          <Icon name="stop" :size="16" />
        </button>
      </Transition>
      <button
        class="composer-send"
        :disabled="!canSend"
        :title="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        :aria-label="isBusy ? t('chat.composer.steer') : t('chat.composer.send')"
        @click="submit"
      >
        <Icon name="send" :size="16" />
      </button>
    </div>
  </div>
</template>
