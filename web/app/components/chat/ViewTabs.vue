<script setup lang="ts">
// The open session's views as tabs along the header's foot: the conversation
// and its raw trajectory. The pick lives in the address (`?view=`), so Back
// and a refresh keep it.
import { t } from '../../i18n'
import type { SessionView } from '../../lib/router'
import { showView, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'

const views: { id: SessionView; icon: string }[] = [
  { id: 'chat', icon: 'chat' },
  { id: 'trace', icon: 'trace' },
]
</script>

<template>
  <!-- -mb-px: the selected tab's underline sits on the header's rule; it
       grows in under the tab chosen as it shrinks away from the last -->
  <div class="-mb-px flex gap-5" role="tablist" :aria-label="t('chat.view.label')">
    <button
      v-for="v in views"
      :key="v.id"
      type="button"
      role="tab"
      class="relative inline-flex items-center gap-1.5 pt-0.5 pb-2.5 text-meta font-semibold transition-colors after:absolute after:inset-x-0 after:bottom-0 after:h-0.5 after:rounded-full after:bg-ink after:transition-[scale] after:duration-300 after:ease-soft"
      :class="
        state.view === v.id ? 'text-ink after:scale-x-100' : 'text-ink-3 after:scale-x-0 hover:text-ink-2'
      "
      :aria-selected="state.view === v.id"
      @click="showView(v.id)"
    >
      <Icon :name="v.icon" :size="14" />
      {{ t(`chat.view.${v.id}`) }}
    </button>
  </div>
</template>
