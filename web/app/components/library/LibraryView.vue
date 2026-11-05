<script setup lang="ts">
// The card library dialog: a folder tree over every deck, with the reader /
// editor beside it. Below lg it is one pane at a time.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { closeLibrary, closeCard, searchLibrary, state } from '../../lib/store'
import RumuCat from '../brand/RumuCat.vue'
import Icon from '../ui/Icon.vue'
import Modal from '../ui/Modal.vue'
import CardEditor from './CardEditor.vue'
import LibraryTree from './LibraryTree.vue'

const searchInput = ref<HTMLInputElement | null>(null)
let opener: HTMLElement | null = null

const query = computed(() => state.library.query.trim())
const count = computed(() => {
  if (!query.value) return String(state.library.cards.length)
  if (state.library.hits === null) return '…'
  return t('library.hits', { n: state.library.hits.length })
})
const showEditor = computed(() => state.library.open !== null)

/** A save that changed the card's deck or path re-opens it at the new spot. */
function onMoved(deck: string, path: string): void {
  if (state.library.open) state.library.open = { deck, path }
}

watch(
  () => state.library.isOpen,
  async (open) => {
    if (open) {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null
      await nextTick()
      searchInput.value?.focus()
    } else {
      opener?.focus()
      opener = null
    }
  },
  { immediate: true },
)

// The search field debounces; `searchLibrary` also drops stale responses.
let searchTimer: ReturnType<typeof setTimeout> | undefined
watch(
  () => state.library.query,
  () => {
    clearTimeout(searchTimer)
    searchTimer = setTimeout(() => void searchLibrary(state.library.query), 200)
  },
)
onBeforeUnmount(() => clearTimeout(searchTimer))
</script>

<template>
  <Modal
    :open="state.library.isOpen"
    :label="t('library.title')"
    :z="57"
    wrapper-class="sm:p-6"
    panel-class="h-dvh w-full sm:h-[min(86dvh,820px)] sm:max-w-[1100px] sm:rounded-sheet"
    @close="closeLibrary"
  >
    <header
      class="flex flex-none flex-wrap items-center gap-x-3 gap-y-2 border-b border-line-soft px-5 py-3 sm:px-6"
    >
      <h2 class="flex items-center gap-2.5 text-title font-bold">
        <span
          class="flex size-8 flex-none items-center justify-center rounded-control bg-memory-soft text-memory"
        >
          <Icon name="memory" :size="16" />
        </span>
        {{ t('library.title') }}
        <span class="rounded-full bg-surface-2 px-2 py-px text-micro font-semibold text-ink-3 tabular-nums">{{
          count
        }}</span>
      </h2>
      <label
        class="order-last flex w-full items-center gap-2 rounded-full border border-transparent bg-surface-2 px-3.5 text-ink-3 transition-colors duration-150 focus-within:border-memory/40 focus-within:bg-surface focus-within:text-memory focus-within:ring-3 focus-within:ring-memory/15 sm:order-none sm:ml-auto sm:w-64"
      >
        <Icon name="search" :size="14" class="flex-none" />
        <span class="sr-only">{{ t('library.search') }}</span>
        <input
          ref="searchInput"
          v-model="state.library.query"
          type="search"
          class="w-full rounded-none border-0 bg-transparent px-0 py-1.5 text-secondary text-ink focus:shadow-none"
          :placeholder="t('library.search')"
        />
      </label>
      <button
        type="button"
        class="icon-btn ml-auto size-8 sm:ml-0"
        :title="t('library.close')"
        :aria-label="t('library.close')"
        @click="closeLibrary"
      >
        <Icon name="close" :size="14" />
      </button>
    </header>

    <div class="grid min-h-0 flex-1 grid-cols-1 lg:grid-cols-[300px_minmax(0,1fr)]">
      <div
        class="flex min-h-0 flex-col bg-canvas/70 lg:border-r lg:border-line-soft"
        :class="showEditor ? 'max-lg:hidden' : ''"
      >
        <LibraryTree />
      </div>
      <div class="flex min-h-0 flex-col" :class="showEditor ? '' : 'max-lg:hidden'">
        <CardEditor
          v-if="state.library.open"
          :deck="state.library.open.deck"
          :path="state.library.open.path"
          :is-new="state.library.open.isNew"
          :in-library="true"
          @close="closeCard"
          @moved="onMoved"
        />
        <div v-else class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
          <RumuCat :size="56" :paper="false" class="opacity-90" />
          <p class="max-w-56 text-secondary text-ink-3">{{ t('library.pick') }}</p>
        </div>
      </div>
    </div>
  </Modal>
</template>
