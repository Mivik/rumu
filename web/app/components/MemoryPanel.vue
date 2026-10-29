<script setup lang="ts">
// Context cards: the cards used on the current path (imported, read or
// written — derived from path nodes) on top, with the full library folded
// into a `Card library` disclosure. Cards whose content changed since the
// branch last saw them get a "changed" hint. Clicking a row opens the card
// drawer and jumps to its latest use.
import { computed } from 'vue'
import { t } from '../i18n'
import type { CardUse } from '../lib/cards'
import { importedBy, latestUseByCard, useIsCurrent } from '../lib/cards'
import { focusNode, openDrawer, state } from '../lib/store'
import Icon from './ui/Icon.vue'
import { layout } from '../lib/layout'

interface Row {
  id: string
  title: string
  description: string
  use: CardUse | null
  updated: boolean
  imported: string | null
  useNodeId: string | null
}

function useLabel(kind: CardUse['kind']): string {
  return t(`cards.use.${kind}`)
}

const rows = computed<Row[]>(() => {
  const latest = latestUseByCard.value
  const list = state.cards.map((c) => {
    const id = c.id
    const use = latest.get(id) ?? null
    return {
      id,
      title: c.title || c.id,
      description: c.description || '',
      use,
      updated: !!use && !useIsCurrent(c, use),
      imported:
        use && use.kind === 'import' && use.node.body.type === 'note'
          ? importedBy(use.node.body.content)
          : null,
      useNodeId: use?.node.id ?? null,
    }
  })
  // Active first, then by path id — siblings of one directory stay together.
  return [...list].sort((a, b) => Number(b.use !== null) - Number(a.use !== null) || a.id.localeCompare(b.id))
})

const activeRows = computed(() => rows.value.filter((r) => r.use))
const otherRows = computed(() => rows.value.filter((r) => !r.use))

function open(r: Row): void {
  openDrawer(r.id, r.useNodeId ?? undefined)
  if (r.useNodeId) focusNode(r.useNodeId)
  // Once the card drawer closes, the timeline should be what's left on
  // narrow screens (the inspector is an overlay there).
  layout.inspectorOpen = false
}
</script>

<template>
  <div class="flex flex-col gap-0.5">
    <div v-if="activeRows.length" class="section-title">{{ t('cards.library.onPath') }}</div>
    <button
      v-for="r in activeRows"
      :key="r.id"
      class="relative flex w-full items-start gap-2 rounded-card bg-surface px-3 py-2 text-left shadow-soft transition-[box-shadow,translate] duration-200 hover:-translate-y-px hover:shadow-card"
      @click="open(r)"
    >
      <span
        aria-hidden="true"
        class="absolute top-1/2 left-0 h-4 w-[3px] -translate-y-1/2 rounded-r-full bg-memory"
      />
      <Icon name="memory" :size="13" class="mt-0.5 text-memory" />
      <span class="flex min-w-0 flex-col gap-px">
        <span class="flex items-center gap-1.5 truncate text-secondary font-semibold text-ink">
          {{ r.title }}
          <span
            v-if="r.updated"
            class="size-1.5 flex-none rounded-full bg-danger"
            :title="t('cards.library.changed')"
          />
        </span>
        <span class="flex gap-2 truncate text-micro text-ink-3">
          <span class="truncate font-mono">{{ useLabel(r.use!.kind) }}</span>
          <span v-if="r.imported" class="truncate">{{ t('cards.library.by', { name: r.imported }) }}</span>
        </span>
      </span>
    </button>

    <details v-if="otherRows.length" class="group mt-2">
      <summary
        class="flex cursor-pointer list-none items-center gap-1.5 rounded-card px-3 py-1.5 select-none hover:bg-surface-2/70 [&::-webkit-details-marker]:hidden"
      >
        <Icon name="chevron" :size="10" class="text-ink-3 transition-transform group-open:rotate-90" />
        <span class="text-meta font-semibold text-ink-2">
          {{ t('cards.library.title', { count: rows.length }) }}
        </span>
      </summary>
      <div class="reveal mt-1 flex flex-col gap-0.5">
        <button
          v-for="r in otherRows"
          :key="r.id"
          class="flex w-full items-start gap-2 rounded-card px-3 py-2 text-left transition-colors hover:bg-surface-2/70"
          :title="r.description"
          @click="open(r)"
        >
          <Icon name="memory" :size="13" class="mt-0.5 text-ink-3" />
          <span class="flex min-w-0 flex-col gap-px">
            <span class="truncate text-secondary font-semibold text-ink">{{ r.title }}</span>
            <span class="flex gap-2 truncate text-micro text-ink-3">
              <span class="truncate font-mono">{{ r.id }}</span>
            </span>
            <span v-if="r.description" class="truncate text-micro text-ink-3">{{ r.description }}</span>
          </span>
        </button>
        <div class="px-3 pt-1 text-micro text-ink-4">
          {{ t('cards.library.total', { count: otherRows.length, total: state.cards.length }) }}
        </div>
      </div>
    </details>
  </div>
</template>
