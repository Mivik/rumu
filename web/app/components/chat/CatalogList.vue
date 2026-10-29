<script setup lang="ts">
// A card catalog note (`memory.catalog`), listed: the cards it offered the
// model, each opening in the CardDrawer, and the folded directories as one
// line each. Shown inside an opened activity run (ActivityRun). The root
// catalog needs no heading — the run's summary already names it; directory
// and workspace catalogs say which slice they are.
import { computed } from 'vue'
import { t } from '../../i18n'
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
  if (!s || s.kind === 'root') return ''
  return s.kind === 'directory'
    ? t('cards.pills.catalogTitles.directory', { prefix: s.prefix })
    : t('cards.pills.catalogTitles.workspace', { name: s.name })
})

function foldedLabel(names: string[]): string {
  const shown = names.slice(0, 5).join(', ')
  return t('cards.pills.moreCards', {
    count: names.length,
    names: `${shown}${names.length > 5 ? '…' : ''}`,
  })
}
</script>

<template>
  <div v-if="catalog" class="text-meta text-ink-2">
    <div v-if="heading" class="mb-1 font-semibold text-ink">{{ heading }}</div>
    <ul class="flex flex-col gap-0.5">
      <li v-for="e in catalog.entries" :key="e.id">
        <button
          type="button"
          class="card-title text-secondary underline-offset-2 hover:underline"
          @click="openDrawer(e.id)"
        >
          {{ e.title }}
        </button>
        <span v-if="e.summary" class="text-ink-3"> — {{ e.summary }}</span>
      </li>
    </ul>
    <div v-if="catalog.folded.length" class="mt-1 flex flex-col gap-0.5 text-ink-3">
      <div v-for="f in catalog.folded" :key="f.prefix">
        <span class="font-mono">{{ f.prefix }}/</span> — {{ foldedLabel(f.names) }}
      </div>
    </div>
  </div>
</template>
