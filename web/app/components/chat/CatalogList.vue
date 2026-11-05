<script setup lang="ts">
// A card catalog note (`memory.catalog`), one per mounted deck: the cards it
// offered the model, each opening in the CardDrawer, and the folded
// directories as one line each. Shown inside an opened activity run
// (ActivityRun), folded under a heading that names the deck's mount point. It
// only mounts when its run opens; v-play-once keeps the cards' underlines
// from drawing again once they have been seen.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { cardLabel } from '../../lib/cards'
import type { CatalogNote, Node } from '../../lib/types'
import { CATALOG_TAG, CATALOG_VERSION } from '../../lib/types'
import { openDrawer } from '../../lib/store'
import { vPlayOnce } from '../../lib/motion'
import Icon from '../ui/Icon.vue'

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
  if (!s?.at) return t('tools.phrase.catalog')
  return t('cards.pills.catalogTitles.deck', { at: s.at })
})

/** Folded until asked for: the run's line is already the outer disclosure. */
const open = ref(false)

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
  <div v-if="catalog" class="text-meta">
    <button
      type="button"
      class="reason-toggle max-w-full"
      :class="open ? 'text-ink-2' : 'text-ink-3 hover:text-ink-2'"
      :aria-expanded="open"
      @click="open = !open"
    >
      <Icon name="memory" :size="12" class="relative top-px flex-none text-memory" />
      <span class="min-w-0 truncate">{{ heading }}</span>
      <Icon name="chevron" :size="10" class="reason-chevron flex-none" :class="{ 'rotate-90': open }" />
    </button>
    <div v-if="open" class="reveal mt-1">
      <ul class="flex flex-col gap-0.5">
        <li v-for="(e, i) in catalog.entries" :key="e.id">
          <button
            v-play-once="{ key: `card:${e.id}` }"
            type="button"
            class="card-ref"
            :style="{ '--i': i }"
            @click="openDrawer(e.id)"
          >
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
  </div>
</template>
