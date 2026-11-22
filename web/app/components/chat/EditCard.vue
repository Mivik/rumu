<script setup lang="ts">
// A file or card the model changed, standing in the conversation rather than
// folded into its run: a header with the path (directory muted; a card's
// link for a card), the +/−
// counts and, for several hunks, how many places it touched; under it the
// diff itself, clipped when long (FileDiff `clip`). The header folds the
// diff away and back.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { ToolRowView } from '../../lib/toolrows'
import { argsOf, toolRowTitle } from '../../lib/toolrows'
import { toolIcon } from '../../lib/toolIcon'
import { useWorkView } from '../../lib/state/appearance'
import Icon from '../ui/Icon.vue'
import CardChip from './CardChip.vue'
import FileDiff from './FileDiff.vue'

/** `joined`: one of an EditList's edits, the box theirs. */
const props = defineProps<{ view: ToolRowView; joined?: boolean }>()
const work = useWorkView()

/** Tri-state: null = as the appearance settings say, until the header is
 *  clicked. */
const manual = ref<boolean | null>(null)
const open = computed(() => manual.value ?? work.value.edits !== 'folded')

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
    class="w-full min-w-0"
    :class="{ 'overflow-hidden rounded-control border border-line': !joined }"
    :data-path-node="view.row.result.id"
  >
    <button
      type="button"
      class="flex w-full min-w-0 items-center gap-2 px-2.5 py-1 text-left transition-colors"
      :class="open ? 'bg-surface-2/60 hover:bg-surface-2' : 'hover:bg-surface-2/70'"
      :title="toolRowTitle(view)"
      :aria-expanded="open"
      @click="manual = !open"
    >
      <Icon :name="toolIcon(view.name)" :size="12" class="flex-none text-tool" />
      <CardChip
        v-if="view.toolCardId"
        class="text-meta"
        :id="view.toolCardId"
        :use-ref="view.cardUseRef ?? undefined"
      />
      <span v-else class="min-w-0 truncate font-mono text-meta" dir="rtl"
        ><bdi
          ><span class="text-ink-3">{{ parts.dir }}</span
          ><span class="text-ink">{{ parts.name }}</span></bdi
        ></span
      >
      <span v-if="view.patch?.kind === 'create'" class="flex-none text-micro text-ink-3">{{
        t('tools.edit.created')
      }}</span>
      <span v-else-if="view.patch?.kind === 'delete'" class="flex-none text-micro text-ink-3">{{
        t('tools.edit.deleted')
      }}</span>
      <span v-if="view.diff" class="flex-none font-mono text-micro">
        <span v-if="view.diff.added || !view.diff.removed" class="font-medium text-pine"
          >+{{ view.diff.added }}</span
        >
        <span v-if="view.diff.removed" class="ml-1 font-medium text-danger">−{{ view.diff.removed }}</span>
      </span>
      <span class="ml-auto flex flex-none items-center gap-2">
        <span v-if="hunks > 1" class="text-micro text-ink-3">{{
          t('tools.edit.hunks', { count: hunks })
        }}</span>
        <Icon
          name="chevron"
          :size="12"
          class="text-ink-3/70 transition-transform duration-100"
          :class="{ 'rotate-90': open }"
        />
      </span>
    </button>
    <FileDiff
      v-if="open"
      clip
      :patch="view.patch"
      :path="view.toolCardId ? 'card.md' : view.callFull"
      :args="argsOf(view.row.call)"
    />
  </div>
</template>
