<script setup lang="ts">
// A tool call waiting on the user (`decision.requested`): questions an `ask`
// puts to the user (QuestionsForm), or approval of an `ask` between
// assistants, mounting a deck, writing to another deck, or a link into
// another deck. It
// unfolds from the top of the composer, so the request sits where the reply
// to it would be written; the oldest pending one of the open session shows,
// the rest wait their turn (other sessions' requests are marked in the
// sidebar). Escape declines (or dismisses the questions), unless focus is in
// a text field.
//
// It never grows past the room the sheet has: the sheet hands it whatever
// height is left above the writing, and the panel caps and scrolls within
// that — so a long request on a phone no longer pushes the writing and its
// send key off the sheet's bottom edge.
//
// The title row folds the panel down to itself: the request stays pending
// (and answers already typed stay put), while the conversation above gets
// the room back. A request that arrives afterwards unfolds it again.
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import { t } from '../../i18n'
import { catOf } from '../../lib/assistantFace'
import { answerDecision, state } from '../../lib/store'
import type { DecisionScope, PendingDecision } from '../../lib/types'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import QuestionsForm from './QuestionsForm.vue'

const pending = computed(() => state.decisions.filter((d) => d.session === state.sessionId))
const cat = computed(() => (state.session ? catOf(state.assistants, state.session.assistant) : undefined))
const current = computed(() => pending.value[0] ?? null)

/** What the panel shows: the current request, or the last one while the
 *  panel folds away after it was answered. */
const shown = shallowRef<PendingDecision | null>(current.value)
/** Folded down to the title row. */
const folded = ref(false)
watch(current, (d) => {
  if (d) {
    shown.value = d
    folded.value = false
  }
})

/** Title, quoted detail (if any) and hint for the shown request. */
interface RequestView {
  title: string
  detail: string | null
  hint: string | null
  /** A workspace entry change, shown side by side. */
  before?: string
  after?: string
}

const view = computed<RequestView | null>(() => {
  const r = shown.value?.request
  if (!r) return null
  switch (r.kind) {
    case 'questions':
      return { title: t('chat.ask.questionsTitle', { from: r.from }), detail: null, hint: null }
    case 'ask':
      return {
        title: t('chat.ask.title', { from: r.from, to: r.to }),
        detail: r.question,
        hint: t('chat.ask.hint', { from: r.from, to: r.to }),
      }
    case 'card_mount':
      if (r.at === '/cards/project')
        return {
          title: t('chat.ask.moveProjectTitle', { from: r.from, project: r.deck }),
          detail: null,
          hint: t('chat.ask.moveProjectHint'),
        }
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
    case 'project_create':
      return {
        title: t('chat.ask.projectCreateTitle', { from: r.from, name: r.name }),
        detail: null,
        hint: null,
      }
    case 'workspace_change':
      return {
        title: t('chat.ask.workspaceChangeTitle', { from: r.from, name: r.name }),
        detail: null,
        hint: null,
        before: r.before,
        after: r.after,
      }
  }
})

function answer(allow: boolean, scope: DecisionScope = 'once'): void {
  if (current.value) void answerDecision(current.value.id, { allow, scope })
}

/** Decline the current request, or dismiss its questions. */
function decline(): void {
  const d = current.value
  if (d) void answerDecision(d.id, d.request.kind === 'questions' ? {} : { allow: false })
}

// Capture phase, before the panels and drawers below it hear the key.
function onKey(e: KeyboardEvent): void {
  const el = document.activeElement
  const typing =
    el instanceof HTMLInputElement ||
    el instanceof HTMLTextAreaElement ||
    (el as HTMLElement | null)?.isContentEditable
  if (e.key === 'Escape' && current.value && !typing) {
    e.stopPropagation()
    decline()
  }
}

onMounted(() => window.addEventListener('keydown', onKey, true))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <!-- 0fr ↔ 1fr rows: the sheet grows upward by exactly the panel's height -->
  <div
    class="grid cursor-auto overflow-hidden rounded-t-[inherit] transition-[grid-template-rows] duration-300 ease-soft"
    :class="current ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'"
    :inert="!current"
  >
    <!-- min-w-0: the folded title is one nowrap line, and without it the row
         sizes the panel to that line instead of the sheet's width -->
    <div class="min-h-0 min-w-0">
      <Transition
        mode="out-in"
        enter-active-class="transition duration-200 ease-soft"
        enter-from-class="translate-y-1 opacity-0"
        leave-active-class="transition duration-100"
        leave-to-class="opacity-0"
      >
        <div
          v-if="shown && view"
          :key="shown.id"
          class="flex h-full max-h-[min(70dvh,560px)] flex-col border-b border-line-soft bg-surface-2/50 px-4 pt-3 pb-3"
          role="alertdialog"
          aria-modal="false"
          :aria-label="view.title"
        >
          <button
            type="button"
            class="flex w-full flex-none items-center gap-2 text-left"
            :aria-expanded="!folded"
            :aria-label="folded ? t('chat.ask.unfold') : t('chat.ask.fold')"
            @click="folded = !folded"
          >
            <RumuCat state="waiting" :cat="cat" :size="34" :paper="false" class="-my-1 flex-none" />
            <span class="min-w-0 flex-1 text-message font-semibold text-ink" :class="{ truncate: folded }">
              {{ view.title }}
            </span>
            <span v-if="pending.length > 1" class="flex-none text-micro text-ink-3 tabular-nums">
              1 / {{ pending.length }}
            </span>
            <Icon
              name="chevron"
              :size="14"
              class="flex-none text-ink-3 transition-transform duration-200"
              :class="folded ? '-rotate-90' : 'rotate-90'"
            />
          </button>
          <div
            class="grid min-h-0 overflow-hidden transition-[grid-template-rows] duration-300 ease-soft"
            :class="folded ? 'grid-rows-[0fr]' : 'grid-rows-[1fr]'"
            :inert="folded"
          >
            <div class="flex min-h-0 flex-col">
              <QuestionsForm
                v-if="shown.request.kind === 'questions'"
                :id="shown.id"
                :request="shown.request"
              />
              <div v-else class="min-h-0 flex-1 overflow-y-auto">
                <div v-if="view.before !== undefined" class="mt-2 grid gap-2 sm:grid-cols-2">
                  <div>
                    <p class="text-micro text-ink-3">{{ t('chat.ask.before') }}</p>
                    <blockquote
                      class="mt-1 max-h-40 overflow-y-auto rounded-control bg-surface px-3 py-2 font-mono text-micro whitespace-pre-wrap text-ink-2 ring-1 ring-line-soft"
                    >
                      {{ view.before }}
                    </blockquote>
                  </div>
                  <div>
                    <p class="text-micro text-ink-3">{{ t('chat.ask.after') }}</p>
                    <blockquote
                      class="mt-1 max-h-40 overflow-y-auto rounded-control bg-surface px-3 py-2 font-mono text-micro whitespace-pre-wrap text-ink-2 ring-1 ring-line-soft"
                    >
                      {{ view.after }}
                    </blockquote>
                  </div>
                </div>
                <blockquote
                  v-else-if="view.detail"
                  class="mt-2 max-h-40 overflow-y-auto rounded-control bg-surface px-3 py-2 text-secondary whitespace-pre-wrap text-ink-2 ring-1 ring-line-soft"
                >
                  {{ view.detail }}
                </blockquote>
                <p v-if="view.hint" class="mt-1.5 text-micro text-ink-3">{{ view.hint }}</p>
                <div class="mt-2.5 flex flex-wrap items-center justify-end gap-1.5">
                  <button
                    type="button"
                    class="btn flex-auto px-3 py-1 sm:mr-auto sm:flex-none"
                    @click="answer(false)"
                  >
                    {{ t('chat.ask.deny') }}
                  </button>
                  <button
                    type="button"
                    class="btn flex-auto px-3 py-1 sm:flex-none"
                    @click="answer(true, 'always')"
                  >
                    {{ t('chat.ask.always') }}
                  </button>
                  <button
                    type="button"
                    class="btn flex-auto px-3 py-1 sm:flex-none"
                    @click="answer(true, 'session')"
                  >
                    {{ t('chat.ask.session') }}
                  </button>
                  <button
                    type="button"
                    class="btn btn-primary flex-auto px-3 py-1 sm:flex-none"
                    @click="answer(true)"
                  >
                    {{ t('chat.ask.once') }}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </Transition>
    </div>
  </div>
</template>
