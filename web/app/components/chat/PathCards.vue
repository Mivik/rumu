<script setup lang="ts">
// Knowledge used on this path. Picking an entry opens its reader and
// focuses its latest use; changed cards retain their status indicator.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { CardUse } from '../../lib/cards'
import { importedBy, latestUseByCard, useIsCurrent } from '../../lib/cards'
import { focusNode, openCatalog, openDrawer, state } from '../../lib/store'

const emit = defineEmits<{ picked: [] }>()

interface Row {
  id: string
  title: string
  description: string
  use: CardUse
  updated: boolean
  imported: string | null
}

const rows = computed<Row[]>(() => {
  const latest = latestUseByCard.value
  const out: Row[] = []
  for (const c of state.cards) {
    const use = latest.get(c.id)
    if (!use) continue
    out.push({
      id: c.id,
      title: c.title || c.id,
      description: c.description || '',
      use,
      updated: !useIsCurrent(c, use),
      imported:
        use.kind === 'import' && use.node.body.type === 'note' ? importedBy(use.node.body.content) : null,
    })
  }
  // By path id — siblings of one directory stay together.
  return out.sort((a, b) => a.id.localeCompare(b.id))
})

function open(r: Row): void {
  openDrawer(r.id, r.use.node.id)
  focusNode(r.use.node.id)
  emit('picked')
}

function browse(): void {
  openCatalog()
  emit('picked')
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <button
      v-for="r in rows"
      :key="r.id"
      type="button"
      class="card-entry flex w-full min-w-0 flex-col p-3 text-left hover:border-memory/40"
      @click="open(r)"
    >
      <span class="flex items-baseline gap-2">
        <span class="min-w-0 line-clamp-2 break-anywhere font-card text-message font-semibold text-ink">{{
          r.title
        }}</span>
        <span
          v-if="r.updated"
          class="size-1.5 flex-none self-center rounded-full bg-danger"
          :title="t('cards.library.changed')"
          ><span class="sr-only">{{ t('cards.library.changed') }}</span></span
        >
      </span>
      <span v-if="r.description" class="mt-1 line-clamp-2 text-secondary leading-relaxed text-ink-2">{{
        r.description
      }}</span>
      <span class="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-meta text-ink-3">
        <span class="text-memory">{{ t(`cards.use.${r.use.kind}`) }}</span>
        <span v-if="r.imported" class="break-anywhere">{{
          t('cards.library.by', { name: r.imported })
        }}</span>
      </span>
    </button>
    <p v-if="!rows.length" class="px-1 py-2 text-meta text-ink-3">{{ t('cards.library.noneOnPath') }}</p>
    <button
      type="button"
      class="mt-1 self-start rounded-control px-1 py-0.5 text-meta font-semibold text-ink-2 underline-offset-4 hover:text-ink hover:underline"
      @click="browse"
    >
      {{ t('cards.library.browse', { count: state.cards.length }) }}
    </button>
  </div>
</template>
