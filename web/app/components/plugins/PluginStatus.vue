<script setup lang="ts">
// The plugins' status items: small labels above the composer, each what its
// `render` gives (text or a vnode; nothing hides it), inside an error
// boundary of its own.
import { computed, defineComponent, h, isVNode } from 'vue'
import { plugins, type StatusItem } from '../../lib/plugins'
import PluginView from './PluginView.vue'

/** One item's content, re-rendered as what it reads changes. */
const Content = defineComponent({
  props: { item: { type: Object as () => StatusItem, required: true } },
  setup(props) {
    return () => {
      const out = props.item.render()
      if (out === null || out === undefined || out === false || out === '') return null
      return isVNode(out) ? out : h('span', String(out))
    }
  },
})

const items = computed(() => plugins.statuses)

/** An item's hover text; a plugin's error costs only the hover. */
function titleOf(item: StatusItem): string | undefined {
  try {
    return item.title?.()
  } catch {
    return undefined
  }
}

function click(item: StatusItem): void {
  try {
    item.onClick?.()
  } catch (err) {
    console.error(`plugin ${item.plugin}:`, err)
  }
}
</script>

<template>
  <div v-if="items.length" class="mx-auto flex w-full max-w-[var(--col-max)] flex-wrap gap-1.5 px-4 pb-1.5">
    <component
      :is="item.onClick ? 'button' : 'span'"
      v-for="item in items"
      :key="`${item.plugin}:${item.id}`"
      :type="item.onClick ? 'button' : undefined"
      class="plugin-status rounded-xs border border-line-soft bg-surface px-1.5 text-micro text-ink-2 empty:hidden"
      :class="{ 'hover:bg-surface-2 hover:text-ink': item.onClick }"
      :title="titleOf(item)"
      @click="click(item)"
    >
      <PluginView :plugin="item.plugin" :is="Content" :props="{ item }" />
    </component>
  </div>
</template>
