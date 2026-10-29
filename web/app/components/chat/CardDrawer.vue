<script setup lang="ts">
// Card detail drawer: opens over the right edge when any card is clicked.
// The card itself — title on the coral rule, its text below. Only states that
// change what you read get a line: a deleted card, or a conversation that saw
// an older version (then tabs switch between that version and the current
// one). Opened from the library, a card the conversation used can be shown
// where it was used.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import type { CardUse } from '../../lib/cards'
import { cardById, cardUsesOnPath, useIsCurrent } from '../../lib/cards'
import { closeCatalog, closeDrawer, focusNode, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'
import { renderMarkdown } from '../../lib/markdown'

const card = computed(() => (state.drawer ? (cardById.value.get(state.drawer.cardId) ?? null) : null))

const keywords = computed(() => card.value?.keywords ?? [])

const uses = computed(() => (state.drawer ? (cardUsesOnPath.value.get(state.drawer.cardId) ?? []) : []))

/** The use this drawer was opened from (a timeline node), else the latest. */
const selectedUse = computed<CardUse | null>(() => {
  const list = uses.value
  if (!list.length) return null
  return list.find((u) => u.node.id === state.drawer?.useNodeId) ?? list[list.length - 1]!
})

const changed = computed(
  () => !!card.value && !!selectedUse.value && !useIsCurrent(card.value, selectedUse.value),
)

const showTabs = computed(() => changed.value && !!card.value)
const tab = ref<'snapshot' | 'current'>('current')
// Opening from a timeline use starts on the snapshot; from the Library, on
// the current card.
watch(
  () => state.drawer,
  (d) => {
    tab.value = d?.useNodeId ? 'snapshot' : 'current'
  },
  { immediate: true },
)

/** A snapshot opens with the card's `# title` line; the header shows that. */
function withoutTitle(md: string): string {
  return md.replace(/^#\s+[^\n]*\n?/, '')
}

const title = computed(
  () =>
    card.value?.title ||
    selectedUse.value?.snapshot.split('\n')[0]?.replace(/^#\s*/, '') ||
    state.drawer?.cardId ||
    '',
)

const shownHtml = computed(() => {
  // cardId makes card-relative links (`./x`, `../x/y.md`) resolve to cards.
  const cardId = state.drawer?.cardId
  if (tab.value === 'snapshot' || !card.value) {
    return renderMarkdown(withoutTitle(selectedUse.value?.snapshot ?? ''), { breaks: false, cardId })
  }
  return renderMarkdown(card.value.body, { breaks: false, cardId })
})

/** Opened from the library (not from a use): offer the way to where it was used. */
const canShow = computed(() => !state.drawer?.useNodeId && !!selectedUse.value)

function jump(nodeId: string): void {
  focusNode(nodeId)
  closeDrawer()
  closeCatalog()
}

function showInTimeline(): void {
  const u = selectedUse.value
  if (u) jump(u.node.id)
}

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape') closeDrawer()
}

onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-opacity duration-300"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-200"
      leave-to-class="opacity-0"
    >
      <div v-if="state.drawer" class="fixed inset-0 z-60 bg-scrim" @click="closeDrawer" />
    </Transition>
    <Transition
      enter-active-class="transition duration-[420ms] ease-soft"
      enter-from-class="translate-x-[calc(100%+8px)]"
      leave-active-class="transition duration-200 ease-in"
      leave-to-class="translate-x-6 opacity-0"
    >
      <aside
        v-if="state.drawer"
        class="fixed top-2 right-2 bottom-2 left-2 z-61 flex w-auto flex-col overflow-hidden rounded-control bg-surface shadow-pop sm:left-auto sm:w-[420px] sm:max-w-[90vw]"
        role="dialog"
        :aria-label="t('cards.drawer.label')"
      >
        <!-- a card is an index card: the title above a coral header rule,
             the card's text below it in the card face -->
        <header class="flex items-end gap-2 border-b-[1.5px] border-brand px-5 pt-4 pb-2.5">
          <span class="card-title min-w-0 flex-1 truncate text-title">{{ title }}</span>
          <span
            v-if="state.drawer.cardId !== title"
            class="max-w-[35%] flex-none truncate pb-0.5 font-card text-meta text-ink-3 italic"
            :title="state.drawer.cardId"
            >{{ state.drawer.cardId }}</span
          >
          <button
            class="icon-btn -mr-1.5 flex-none"
            :title="t('cards.drawer.close')"
            :aria-label="t('cards.drawer.closeAria')"
            @click="closeDrawer"
          >
            <Icon name="close" :size="13" />
          </button>
        </header>

        <div v-if="keywords.length" class="flex flex-wrap gap-1.5 px-5 pt-3">
          <span v-for="(k, i) in keywords" :key="i" class="keyword-tag">{{ k }}</span>
        </div>

        <p v-if="!card" class="px-5 pt-3 text-meta text-ink-3">{{ t('cards.drawer.deleted') }}</p>
        <div v-else-if="showTabs" class="flex flex-wrap items-center gap-x-3 gap-y-2 px-5 pt-3">
          <span class="text-meta text-ink-3">{{ t('cards.drawer.olderSeen') }}</span>
          <span class="flex gap-1">
            <button
              class="rounded-control px-2 py-1 text-micro font-semibold transition-colors"
              :class="tab === 'snapshot' ? 'bg-surface-2 text-ink' : 'text-ink-3 hover:bg-surface-2'"
              @click="tab = 'snapshot'"
            >
              {{ t('cards.drawer.asSeen') }}
            </button>
            <button
              class="rounded-control px-2 py-1 text-micro font-semibold transition-colors"
              :class="tab === 'current' ? 'bg-surface-2 text-ink' : 'text-ink-3 hover:bg-surface-2'"
              @click="tab = 'current'"
            >
              {{ t('cards.drawer.current') }}
            </button>
          </span>
        </div>

        <div class="cascade-md md card-face flex-1 overflow-y-auto px-5 py-4 text-ink-2" v-html="shownHtml" />

        <footer v-if="canShow" class="border-t border-line-soft px-5 py-3">
          <button class="btn w-full" @click="showInTimeline">
            {{ t('cards.drawer.showInTimeline') }}
          </button>
        </footer>
      </aside>
    </Transition>
  </Teleport>
</template>
