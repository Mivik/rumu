<script setup lang="ts">
// The cards the open path has used (imported, read or written — derived from
// path nodes), as index cards: title on the coral rule, how the path used it
// and what the card is about on the ruled lines. Cards whose content changed
// since the branch last saw them get a "changed" dot. Clicking one opens the
// card drawer and jumps the timeline to its latest use. Lives in the chat
// header's cards popover; the whole library is the CardCatalog.
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
      class="index-card flex w-full flex-col px-3 pb-2 text-left transition-[box-shadow,translate] duration-200 [--card-head:28px] hover:-translate-y-px"
      @click="open(r)"
    >
      <span class="flex h-[28px] items-end gap-1.5 pb-1">
        <span class="card-title min-w-0 truncate text-secondary">{{ r.title }}</span>
        <span
          v-if="r.updated"
          class="mb-1.5 size-1.5 flex-none rounded-full bg-danger"
          :title="t('cards.library.changed')"
        />
      </span>
      <span class="mt-[1.5px] flex h-5 items-center gap-2 text-micro text-ink-3">
        <span class="flex-none font-bold text-memory">{{ t(`cards.use.${r.use.kind}`) }}</span>
        <span v-if="r.imported" class="truncate">{{ t('cards.library.by', { name: r.imported }) }}</span>
      </span>
      <span v-if="r.description" class="h-5 truncate font-card text-micro leading-5 text-ink-2">{{
        r.description
      }}</span>
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
