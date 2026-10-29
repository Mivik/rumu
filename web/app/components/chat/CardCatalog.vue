<script setup lang="ts">
// The card catalog: the whole library pulled out over the chat, every card an
// index card on one sheet, with a filter over titles, descriptions, paths and
// keywords. Opened from the sidebar or the cards popover; a card opens in the
// CardDrawer on top of it. Escape closes the drawer first, then this.
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { t } from '../../i18n'
import { closeCatalog, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'
import IndexCard from './IndexCard.vue'

const query = ref('')
const input = ref<HTMLInputElement | null>(null)

const shown = computed(() => {
  const q = query.value.trim().toLowerCase()
  if (!q) return state.cards
  return state.cards.filter((c) =>
    [c.id, c.title ?? '', c.description ?? '', ...(c.keywords ?? [])].some((s) =>
      s.toLowerCase().includes(q),
    ),
  )
})

// Capture phase: this runs before the drawer's own Escape handler, so an open
// drawer still shows here and takes the key alone.
function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && !state.drawer) closeCatalog()
}

watch(
  () => state.catalogOpen,
  async (open) => {
    if (open) {
      query.value = ''
      window.addEventListener('keydown', onKey, true)
      await nextTick()
      input.value?.focus()
    } else {
      window.removeEventListener('keydown', onKey, true)
    }
  },
)

onBeforeUnmount(() => window.removeEventListener('keydown', onKey, true))
</script>

<template>
  <Teleport to="body">
    <Transition
      enter-active-class="transition-opacity duration-300"
      enter-from-class="opacity-0"
      leave-active-class="transition-opacity duration-200"
      leave-to-class="opacity-0"
    >
      <div v-if="state.catalogOpen" class="fixed inset-0 z-55 bg-scrim" @click="closeCatalog" />
    </Transition>
    <Transition
      enter-active-class="transition-[opacity,translate] duration-300 ease-soft"
      enter-from-class="translate-y-4 opacity-0"
      leave-active-class="transition-[opacity,translate] duration-150 ease-in"
      leave-to-class="translate-y-2 opacity-0"
    >
      <!-- centered, as tall as its cards need up to a cap; the scrim behind
           takes clicks around it -->
      <div
        v-if="state.catalogOpen"
        class="pointer-events-none fixed inset-0 z-56 flex items-center justify-center p-3 sm:p-6"
      >
        <section
          role="dialog"
          :aria-label="t('cards.library.title')"
          class="pointer-events-auto flex max-h-[min(80vh,620px)] w-full max-w-[860px] flex-col overflow-hidden rounded-sheet bg-surface shadow-pop"
        >
          <header
            class="flex flex-none flex-wrap items-center gap-x-4 gap-y-3 border-b border-line-soft px-5 py-4 sm:px-7"
          >
            <h2 class="flex items-baseline gap-2 text-title font-bold">
              {{ t('cards.library.title') }}
              <span class="text-meta font-semibold text-ink-3 tabular-nums">{{ state.cards.length }}</span>
            </h2>
            <label class="order-last flex w-full items-center sm:order-none sm:ml-auto sm:w-72">
              <span class="sr-only">{{ t('cards.catalog.search') }}</span>
              <input
                ref="input"
                v-model="query"
                type="search"
                class="w-full rounded-full bg-surface-2 px-4 py-1.5 text-secondary"
                :placeholder="t('cards.catalog.search')"
              />
            </label>
            <button
              class="icon-btn ml-auto size-8 sm:ml-0"
              :title="t('common.close')"
              :aria-label="t('common.close')"
              @click="closeCatalog"
            >
              <Icon name="close" :size="14" />
            </button>
          </header>

          <div class="min-h-0 overflow-y-auto bg-canvas/60 px-5 py-6 sm:px-7">
            <div v-if="shown.length" class="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
              <IndexCard v-for="c in shown" :key="c.id" :card="c" class="catalog-card" />
            </div>
            <p v-else class="py-16 text-center text-secondary text-ink-3">
              {{
                state.cards.length ? t('cards.catalog.none', { q: query.trim() }) : t('cards.catalog.empty')
              }}
            </p>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.catalog-card {
  transition:
    translate 0.2s var(--ease-soft),
    box-shadow 0.2s var(--ease-soft);
}

.catalog-card:hover,
.catalog-card:focus-visible {
  translate: 0 -2px;
}
</style>
