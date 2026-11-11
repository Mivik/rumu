<script setup lang="ts">
// Where a compaction happened: from here on the model sees its summary in
// place of the older messages. The line names the size before and after; it
// opens to the summary itself.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { compactionOf } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import { vEnter } from '../../lib/motion'
import type { Node } from '../../lib/types'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ node: Node }>()

const open = ref(false)
const data = computed(() => compactionOf(props.node))

const tokens = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 })

/** The summary alone: the note also carries the user's own messages, which
 *  the timeline shows already. */
const summary = computed(() => {
  const b = props.node.body
  const text = b.type === 'note' ? b.content.text : ''
  const m = /<summary>\n?([\s\S]*?)\n?<\/summary>/.exec(text)
  return m?.[1] ?? text
})
</script>

<template>
  <div class="flex min-w-0 flex-col gap-2">
    <button
      type="button"
      class="group/cline flex w-full min-w-0 items-center gap-3 text-meta text-ink-3 transition-colors hover:text-ink"
      :aria-expanded="open"
      :title="open ? t('chat.compaction.hide') : t('chat.compaction.show')"
      @click="open = !open"
    >
      <span class="h-px flex-1 bg-line-soft" />
      <span class="flex flex-none items-center gap-1.5 tabular-nums">
        <Icon name="history" :size="12" />
        {{
          t('chat.compaction.line', {
            before: tokens.format(data?.before ?? 0),
            after: tokens.format(data?.after ?? 0),
          })
        }}
        <Icon
          name="down"
          :size="12"
          class="transition-transform duration-150"
          :class="open ? 'rotate-180' : ''"
        />
      </span>
      <span class="h-px flex-1 bg-line-soft" />
    </button>
    <div
      v-if="open"
      v-enter="'enter-fade'"
      class="md rounded-control border border-line-soft bg-surface-2 px-4 py-3 text-meta text-ink-2"
      v-html="renderMarkdown(summary)"
    />
  </div>
</template>
