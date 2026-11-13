<script setup lang="ts">
// A file the model changed, standing in the conversation rather than folded
// into its run: a header with the path (directory muted), the +/− counts
// and, for a diff of several hunks, how many places it touched; under it the
// diff itself, clipped when long (FileDiff `clip`). The header folds the
// diff away and back.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { ToolRowView } from '../../lib/toolrows'
import { argsOf, toolRowTitle } from '../../lib/toolrows'
import Icon from '../ui/Icon.vue'
import FileDiff from './FileDiff.vue'

const props = defineProps<{ view: ToolRowView }>()

const open = ref(true)

/** `ws:dir/name` → the muted head (`ws:dir/`) and the name. */
const parts = computed(() => {
  const path = props.view.callFull
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf(':')) + 1
  return { dir: path.slice(0, cut), name: path.slice(cut) || path }
})

const hunks = computed(() => props.view.patch?.hunks?.length ?? 0)
</script>

<template>
  <div
    class="w-full min-w-0 overflow-hidden rounded-control border border-line"
    :data-path-node="view.row.result.id"
  >
    <button
      type="button"
      class="flex w-full min-w-0 items-center gap-2 px-2.5 py-1 text-left transition-colors"
      :class="open ? 'bg-surface-2/60 hover:bg-surface-2' : 'hover:bg-surface-2/70'"
      :title="toolRowTitle(view)"
      :aria-expanded="open"
      @click="open = !open"
    >
      <Icon name="tool" :size="12" class="flex-none text-tool" />
      <span class="min-w-0 truncate font-mono text-meta" dir="rtl"
        ><bdi
          ><span class="text-ink-3">{{ parts.dir }}</span
          ><span class="text-ink">{{ parts.name }}</span></bdi
        ></span
      >
      <span v-if="view.patch?.kind === 'create'" class="flex-none text-micro text-ink-3">{{
        t('tools.edit.created')
      }}</span>
      <span v-if="view.diff" class="flex-none font-mono text-micro">
        <span class="font-medium text-pine">+{{ view.diff.added }}</span>
        <span v-if="view.diff.removed" class="ml-1 font-medium text-danger">−{{ view.diff.removed }}</span>
      </span>
      <span v-if="hunks > 1" class="ml-auto flex-none text-micro text-ink-3">{{
        t('tools.edit.hunks', { count: hunks })
      }}</span>
      <Icon
        name="chevron"
        :size="12"
        class="flex-none text-ink-3/70 transition-transform duration-100"
        :class="[{ 'rotate-90': open }, hunks > 1 ? '' : 'ml-auto']"
      />
    </button>
    <FileDiff v-if="open" clip :patch="view.patch" :path="view.callFull" :args="argsOf(view.row.call)" />
  </div>
</template>
