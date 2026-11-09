<script setup lang="ts">
// The card library page: a folder tree over every deck, with the reader /
// editor of the open card beside it (`/library/<deck>/<path>`).
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { pageUp } from '../../lib/router'
import { moveOpenCard, openedCard, searchLibrary, state } from '../../lib/store'
import CardEditor from '../library/CardEditor.vue'
import LibraryTree from '../library/LibraryTree.vue'
import Icon from '../ui/Icon.vue'
import PageSheet from './PageSheet.vue'

const editor = ref<InstanceType<typeof CardEditor> | null>(null)

const query = computed(() => state.library.query.trim())
const count = computed(() => {
  if (!query.value) return String(state.library.cards.length)
  if (state.library.hits === null) return '…'
  return t('library.hits', { n: state.library.hits.length })
})

/** The narrow sheet's back arrow: leave the card, asking about unsaved edits. */
function back(): void {
  if (editor.value?.confirmLeave() ?? true) pageUp()
}

/** A save that changed the card's deck or path re-opens it at the new spot. */
function onMoved(deck: string, path: string): void {
  moveOpenCard({ deck, path })
}

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
  <PageSheet
    :title="t('library.title')"
    :count="count"
    icon="memory"
    tone="text-memory"
    :detail="openedCard !== null"
    :pick="t('library.pick')"
    @back="back"
  >
    <template #list>
      <label
        class="mx-3 mt-3 flex flex-none items-center gap-2 rounded-full border border-transparent bg-surface-2 px-3.5 text-ink-3 transition-colors duration-150 focus-within:border-memory/40 focus-within:bg-surface focus-within:text-memory focus-within:ring-3 focus-within:ring-memory/15"
      >
        <Icon name="search" :size="14" class="flex-none" />
        <span class="sr-only">{{ t('library.search') }}</span>
        <input
          v-model="state.library.query"
          type="search"
          class="w-full rounded-none border-0 bg-transparent px-0 py-1.5 text-secondary text-ink focus:shadow-none"
          :placeholder="t('library.search')"
        />
      </label>
      <LibraryTree />
    </template>

    <CardEditor
      v-if="openedCard"
      ref="editor"
      :deck="openedCard.deck"
      :path="openedCard.path"
      :is-new="openedCard.isNew"
      :in-library="true"
      @close="pageUp"
      @moved="onMoved"
    />
  </PageSheet>
</template>
