<script setup lang="ts">
// Approval for a tool call (`decision.requested`): an `ask` between assistants,
// mounting a deck, writing to another deck, or a link into another deck. Mounted at the app
// root so a request from any session is seen; the oldest pending one shows, the
// rest wait their turn. Escape declines, unless focus is in a text field.
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { t } from '../i18n'
import { answerDecision, state } from '../lib/store'
import type { DecisionScope } from '../lib/types'
import RumuCat from './brand/RumuCat.vue'

const current = computed(() => state.decisions[0] ?? null)

/** Title, quoted detail (if any) and hint for the pending request. */
const view = computed(() => {
  const r = current.value?.request
  if (!r) return null
  switch (r.kind) {
    case 'ask':
      return {
        title: t('chat.ask.title', { from: r.from, to: r.to }),
        detail: r.question,
        hint: t('chat.ask.hint', { from: r.from, to: r.to }),
      }
    case 'card_mount':
      return {
        title: t('chat.ask.mountTitle', { from: r.from, deck: r.deck }),
        detail: null,
        hint: t('chat.ask.mountHint', { deck: r.deck, at: r.at }),
      }
    case 'card_write':
      return {
        title: t('chat.ask.writeTitle', { from: r.from, deck: r.deck }),
        detail: r.content,
        hint: r.move_from
          ? t('chat.ask.moveHint', { path: r.path, source: r.move_from })
          : t('chat.ask.writeHint', { path: r.path }),
      }
    case 'card_link':
      return {
        title: t('chat.ask.linkTitle', { from: r.from, to: r.to }),
        detail: `${r.path} → ${r.target}`,
        hint: t('chat.ask.linkHint', { path: r.path, target: r.target }),
      }
  }
})

function answer(allow: boolean, scope: DecisionScope = 'once'): void {
  if (current.value) void answerDecision(current.value.id, { allow, scope })
}

// Capture phase, before the panels and drawers below it hear the key.
function onKey(e: KeyboardEvent): void {
  const el = document.activeElement
  const typing = el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || (el as HTMLElement | null)?.isContentEditable
  if (e.key === 'Escape' && current.value && !typing) {
    e.stopPropagation()
    answer(false)
  }
}

onMounted(() => window.addEventListener('keydown', onKey, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition duration-300 ease-spring"
      enter-from-class="translate-y-3 scale-95 opacity-0"
      leave-active-class="transition duration-200 ease-in"
      leave-to-class="opacity-0"
    >
      <div
        v-if="current && view"
        :key="current.id"
        class="fixed inset-x-4 bottom-4 z-75 mx-auto max-w-[440px] rounded-card border border-line-soft bg-surface p-4 shadow-pop"
        role="alertdialog"
        aria-modal="false"
        :aria-label="view.title"
      >
        <div class="flex items-center gap-2">
          <RumuCat state="waiting" :size="26" :paper="false" />
          <p class="text-secondary font-semibold text-ink">
            {{ view.title }}
          </p>
          <span v-if="state.decisions.length > 1" class="ml-auto text-micro text-ink-3 tabular-nums">
            1 / {{ state.decisions.length }}
          </span>
        </div>
        <blockquote
          v-if="view.detail"
          class="mt-3 max-h-40 overflow-y-auto rounded-control bg-surface-2 px-3 py-2 text-secondary whitespace-pre-wrap text-ink-2"
        >
          {{ view.detail }}
        </blockquote>
        <p class="mt-2 text-micro text-ink-3">
          {{ view.hint }}
        </p>
        <div class="mt-3 flex flex-wrap items-center justify-end gap-2">
          <button type="button" class="btn mr-auto" @click="answer(false)">{{ t('chat.ask.deny') }}</button>
          <button type="button" class="btn" @click="answer(true, 'always')">{{ t('chat.ask.always') }}</button>
          <button type="button" class="btn" @click="answer(true, 'session')">{{ t('chat.ask.session') }}</button>
          <button type="button" class="btn btn-primary" @click="answer(true)">
            {{ t('chat.ask.once') }}
          </button>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>
