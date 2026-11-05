<script setup lang="ts">
// A card catalog note (`memory.catalog`), one per mounted deck: the cards it
// offered the model, each opening in the CardDrawer, and the folded
// directories as one line each. Shown inside an opened activity run
// (ActivityRun); the heading names the deck's mount point. It only mounts when
// its run opens, so its cards' underlines draw in each time
// (.underline-cascade).
import { computed } from 'vue'
import { t } from '../../i18n'
import { cardLabel } from '../../lib/cards'
import type { CatalogNote, Node } from '../../lib/types'
import { CATALOG_TAG, CATALOG_VERSION } from '../../lib/types'
import { openDrawer } from '../../lib/store'

const props = defineProps<{ node: Node }>()

const catalog = computed<CatalogNote | null>(() => {
  const n = props.node
  if (n.body.type !== 'note') return null
  const d = n.body.content.data
  if (!d || d.tag !== CATALOG_TAG || d.v !== CATALOG_VERSION) return null
  const c = d.data as CatalogNote
  return c && typeof c === 'object' && c.scope && Array.isArray(c.entries) ? c : null
})

const heading = computed(() => {
  const s = catalog.value?.scope
  if (!s?.at) return ''
  return t('cards.pills.catalogTitles.deck', { at: s.at })
})

/** A folded directory without its mount prefix (`self/dev` → `dev/`). */
function folderLabel(prefix: string): string {
  const label = cardLabel(prefix)
  return label ? `${label}/` : ''
}

function foldedLabel(names: string[]): string {
  const shown = names.slice(0, 5).join(', ')
  return t('cards.pills.moreCards', {
    count: names.length,
    names: `${shown}${names.length > 5 ? '…' : ''}`,
  })
}
</script>

<template>
  <div v-if="catalog" class="underline-cascade text-meta">
    <div v-if="heading" class="mb-1 font-semibold text-ink">{{ heading }}</div>
    <ul class="flex flex-col gap-0.5">
      <li v-for="(e, i) in catalog.entries" :key="e.id">
        <button type="button" class="card-ref" :style="{ '--i': i }" @click="openDrawer(e.id)">
          {{ cardLabel(e.id) }}
        </button>
        <span v-if="e.summary" class="text-ink-3"> — {{ e.summary }}</span>
      </li>
    </ul>
    <div v-if="catalog.folded.length" class="mt-1 flex flex-col gap-0.5 text-ink-3">
      <div v-for="f in catalog.folded" :key="f.prefix">
        <template v-if="folderLabel(f.prefix)">
          <span class="font-mono">{{ folderLabel(f.prefix) }}</span> —
        </template>
        {{ foldedLabel(f.names) }}
      </div>
    </div>
  </div>
</template>
