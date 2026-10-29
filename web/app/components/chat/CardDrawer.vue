<script setup lang="ts">
// Card detail drawer: opens over the right edge when any card chip is
// clicked. Shows the current card or the snapshot the branch actually saw,
// with a tab toggle when the two differ, plus the full list of uses on this
// path (imports, reads, writes) as timeline jump targets.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import type { CardUse } from '../../lib/cards'
import { cardById, cardUsesOnPath, importedBy, renderCard, useIsCurrent } from '../../lib/cards'
import { closeDrawer, focusNode, state } from '../../lib/store'
import { layout } from '../../lib/layout'
import Icon from '../ui/Icon.vue'
import Chip from '../ui/Chip.vue'
import { renderMarkdown } from '../../lib/markdown'

const card = computed(() => (state.drawer ? (cardById.value.get(state.drawer.cardId) ?? null) : null))

const uses = computed(() => (state.drawer ? (cardUsesOnPath.value.get(state.drawer.cardId) ?? []) : []))

/** The use this drawer was opened from (a timeline node), else the latest. */
const selectedUse = computed<CardUse | null>(() => {
  const list = uses.value
  if (!list.length) return null
  return list.find((u) => u.node.id === state.drawer?.useNodeId) ?? list[list.length - 1]!
})

const currentRev = computed(() => card.value?.revision ?? null)
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

const shownHtml = computed(() => {
  // cardId makes card-relative links (`./x`, `../x/y.md`) resolve to cards.
  const cardId = state.drawer?.cardId
  if (tab.value === 'snapshot' || !card.value) {
    return renderMarkdown(selectedUse.value?.snapshot ?? '', { breaks: false, cardId })
  }
  return renderMarkdown(renderCard(card.value), { breaks: false, cardId })
})

function useLabel(kind: CardUse['kind']): string {
  return t(`cards.use.${kind}`)
}

const importedFrom = computed(() => {
  const u = selectedUse.value
  if (!u || u.kind !== 'import') return null
  const n = u.node
  if (n.body.type !== 'note') return null
  return importedBy(n.body.content)
})

function jump(nodeId: string): void {
  focusNode(nodeId)
  closeDrawer()
  layout.inspectorOpen = false
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
        class="fixed top-2 right-2 bottom-2 left-2 z-61 flex w-auto flex-col overflow-hidden rounded-card border border-line-soft bg-surface shadow-pop sm:left-auto sm:w-[400px] sm:max-w-[90vw]"
        role="dialog"
        :aria-label="t('cards.drawer.label')"
      >
        <!-- a card is a note card: a memory-colored ribbon across its top edge -->
        <span aria-hidden="true" class="h-1 flex-none bg-memory/70" />
        <header class="flex items-center gap-2 border-b border-line-soft px-5 py-3.5">
          <Icon name="memory" :size="15" class="text-memory" />
          <span class="min-w-0 flex-1 truncate font-display text-title font-semibold">{{
            card?.title || selectedUse?.snapshot.split('\n')[0]?.replace(/^#\s*/, '') || state.drawer.cardId
          }}</span>
          <button
            class="icon-btn"
            :title="t('cards.drawer.close')"
            :aria-label="t('cards.drawer.closeAria')"
            @click="closeDrawer"
          >
            <Icon name="close" :size="13" />
          </button>
        </header>

        <div class="flex flex-wrap items-center gap-1.5 px-5 pt-3">
          <span class="font-mono text-meta text-ink-3" :title="state.drawer.cardId"
            >#{{ state.drawer.cardId }}</span
          >
          <Chip v-if="!card" icon="close" tone="neutral" :label="t('cards.drawer.deleted')" />
          <template v-else-if="!uses.length">
            <Chip icon="note" tone="neutral" :label="t('cards.drawer.notOnPath')" />
          </template>
          <template v-else>
            <Chip
              :icon="changed ? 'note' : 'ok'"
              :tone="changed ? 'danger' : 'memory'"
              :label="changed ? t('cards.drawer.onPathChanged') : t('cards.drawer.onPathCurrent')"
            />
          </template>
        </div>

        <div v-if="importedFrom" class="flex items-baseline gap-2 px-5 pt-1.5 text-micro">
          <span class="text-ink-3">{{ t('cards.drawer.importedBy') }}</span>
          <span class="font-mono text-ink-2">{{ importedFrom }}</span>
        </div>

        <div v-if="showTabs" class="flex gap-1 px-5 pt-3">
          <button
            class="rounded-control px-2 py-1 text-micro font-semibold transition-colors"
            :class="tab === 'snapshot' ? 'bg-memory-soft text-memory' : 'text-ink-3 hover:bg-surface-2'"
            @click="tab = 'snapshot'"
          >
            {{
              selectedUse?.rev
                ? t('cards.drawer.asSeenRev', { rev: selectedUse.rev.slice(0, 4) })
                : t('cards.drawer.asSeen')
            }}
          </button>
          <button
            class="rounded-control px-2 py-1 text-micro font-semibold transition-colors"
            :class="tab === 'current' ? 'bg-memory-soft text-memory' : 'text-ink-3 hover:bg-surface-2'"
            @click="tab = 'current'"
          >
            {{
              currentRev
                ? t('cards.drawer.currentRev', { rev: currentRev.slice(0, 4) })
                : t('cards.drawer.current')
            }}
          </button>
        </div>

        <div
          class="cascade-md md flex-1 overflow-y-auto px-5 py-4 text-secondary text-ink-2"
          v-html="shownHtml"
        />

        <footer v-if="uses.length" class="border-t border-line-soft px-5 py-3">
          <div class="mb-1.5 text-micro font-semibold text-ink-3">
            {{ t('cards.drawer.onPath') }}
          </div>
          <div class="flex flex-col gap-0.5">
            <button
              v-for="u in uses"
              :key="u.node.id"
              class="flex items-center gap-2 rounded-control px-2 py-1 text-left text-micro text-ink-2 transition-colors hover:bg-surface-2/70"
              @click="jump(u.node.id)"
            >
              <Icon name="memory" :size="11" class="flex-none text-memory" />
              <span class="font-mono">{{ useLabel(u.kind) }}</span>
              <span v-if="u.rev" class="font-mono text-ink-3">{{
                t('cards.drawer.rev', { rev: u.rev.slice(0, 8) })
              }}</span>
              <span class="ml-auto flex items-center gap-1 text-ink-3">
                {{ t('common.show') }}
                <Icon name="external" :size="10" />
              </span>
            </button>
          </div>
        </footer>
        <footer v-else-if="selectedUse" class="border-t border-line-soft px-5 py-3">
          <button class="btn w-full" @click="showInTimeline">
            <Icon name="external" :size="12" />
            {{ t('cards.drawer.showInTimeline') }}
          </button>
        </footer>
      </aside>
    </Transition>
  </Teleport>
</template>
