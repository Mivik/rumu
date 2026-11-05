<script setup lang="ts">
// The cards recalled for a step (`memory.import`), listed under their count
// by mount-free label inside an opened activity run. Each opens in the
// CardDrawer.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { cardForImport, cardLabel, importOf } from '../../lib/cards'
import { openDrawer } from '../../lib/store'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ nodes: Node[] }>()

interface Recalled {
  node: Node
  id: string | null
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
    const key = id ?? `${imp.deck}:${imp.path}`
    if (seen.has(key)) continue
    seen.add(key)
    const label = card ? cardLabel(card.id) : imp.path
    const what = card?.description ? ` — ${card.description}` : ''
    out.push({ node: n, id, label, hover: `#${label}${what}` })
  }
  return out
})
</script>

<template>
  <div
    v-if="items.length"
    class="underline-cascade flex flex-wrap items-baseline gap-x-1.5 gap-y-1 text-meta"
  >
    <span class="inline-flex items-baseline gap-1.5 text-micro font-semibold text-ink-3">
      <Icon name="memory" :size="12" class="relative top-px flex-none self-center text-memory" />
      <span>{{ t('cards.pills.recalledList', { count: items.length }) }}</span>
    </span>
    <template v-for="(c, i) in items" :key="c.node.id">
      <span class="inline-flex min-w-0 items-baseline">
        <button
          v-if="c.id"
          type="button"
          class="card-ref"
          :style="{ '--i': i }"
          :title="c.hover"
          @click="openDrawer(c.id)"
        >
          <span class="truncate">{{ c.label }}</span>
        </button>
        <span v-else class="card-ref" :style="{ '--i': i }" :title="c.hover">
          <span class="truncate">{{ c.label }}</span>
        </span>
        <span v-if="i < items.length - 1" class="text-ink-3">,</span>
      </span>
    </template>
  </div>
</template>
