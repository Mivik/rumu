<script setup lang="ts">
// The cards recalled for a step (`memory.import`), listed under their count
// by their mounted id inside an opened activity run. Each opens in the
// CardDrawer. A card's underline draws once, the first time it is seen.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { cardForImport, importOf, mountedId } from '../../lib/cards'
import { openDrawer } from '../../lib/store'
import { vPlayOnce } from '../../lib/motion'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ nodes: Node[] }>()

interface Recalled {
  node: Node
  id: string | null
  key: string
  label: string
  hover: string
}

/** One entry per card, in recall order. */
const items = computed<Recalled[]>(() => {
  const out: Recalled[] = []
  const seen = new Set<string>()
  for (const n of props.nodes) {
    if (n.body.type !== 'note') continue
    const imp = importOf(n.body.content)
    if (!imp) continue
    const card = cardForImport(imp)
    const id = card?.id ?? null
    // The card id when it is in the tree; otherwise the import's own path.
    const key = `card:${id ?? `${imp.deck}:${imp.path}`}`
    if (seen.has(key)) continue
    seen.add(key)
    const label = card?.id ?? mountedId(imp.deck, imp.path) ?? imp.path
    const what = card?.description ? ` — ${card.description}` : ''
    out.push({ node: n, id, key, label, hover: `#${label}${what}` })
  }
  return out
})
</script>

<template>
  <div v-if="items.length" class="flex flex-wrap items-baseline gap-x-1.5 gap-y-1 text-meta">
    <span class="inline-flex items-baseline gap-1.5 text-micro font-semibold text-ink-3">
      <Icon name="memory" :size="12" class="text-memory" />
      <span>{{ t('cards.pills.recalledList', { count: items.length }) }}</span>
    </span>
    <template v-for="(c, i) in items" :key="c.node.id">
      <span class="inline-flex min-w-0 items-baseline">
        <button
          v-if="c.id"
          v-play-once="{ key: c.key }"
          type="button"
          class="card-ref"
          :style="{ '--i': i }"
          :title="c.hover"
          @click="openDrawer(c.id)"
        >
          <span class="truncate">{{ c.label }}</span>
        </button>
        <span v-else v-play-once="{ key: c.key }" class="card-ref" :style="{ '--i': i }" :title="c.hover">
          <span class="truncate">{{ c.label }}</span>
        </span>
        <span v-if="i < items.length - 1" class="text-ink-3">,</span>
      </span>
    </template>
  </div>
</template>
