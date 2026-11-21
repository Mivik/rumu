<script setup lang="ts">
// Edits standing together — those outside a folded run, or a stretch of
// them between its calls when opened: one box, hairlines between, like a
// run of calls (ToolList), an EditCard each, in order. With `limit`, past a
// few the rest wait behind one "N more edits" row.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import type { ToolRowView } from '../../lib/toolrows'
import Icon from '../ui/Icon.vue'
import EditCard from './EditCard.vue'

const props = defineProps<{ views: ToolRowView[]; limit?: boolean }>()

/** At most this many rows; past it, one fewer edit and the "more" row. */
const ROWS = 5

const all = ref(false)
const shown = computed(() =>
  all.value || !props.limit || props.views.length <= ROWS ? props.views : props.views.slice(0, ROWS - 1),
)
</script>

<template>
  <div class="flex flex-col divide-y divide-line overflow-hidden rounded-control border border-line">
    <EditCard v-for="v in shown" :key="v.row.result.id" :view="v" joined />
    <button
      v-if="shown.length < views.length"
      type="button"
      class="flex w-full items-center gap-1 px-2.5 py-1 text-left text-meta text-ink-3 transition-colors hover:bg-surface-2/70 hover:text-ink-2"
      @click="all = true"
    >
      {{ t('tools.edit.moreEdits', { count: views.length - shown.length }) }}
      <Icon name="chevron" :size="12" class="flex-none" />
    </button>
  </div>
</template>
