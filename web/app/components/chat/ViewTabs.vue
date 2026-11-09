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
  <!-- -mb-px: the selected tab's underline sits on the header's rule -->
  <div class="-mb-px flex gap-5" role="tablist" :aria-label="t('chat.view.label')">
    <button
      v-for="v in views"
      :key="v.id"
      type="button"
      role="tab"
      class="inline-flex items-center gap-1.5 border-b-2 pt-0.5 pb-2 text-meta font-semibold transition-colors"
      :class="state.view === v.id ? 'border-ink text-ink' : 'border-transparent text-ink-3 hover:text-ink-2'"
      :aria-selected="state.view === v.id"
      @click="showView(v.id)"
    >
      <Icon :name="v.icon" :size="14" />
      {{ t(`chat.view.${v.id}`) }}
    </button>
  </div>
</template>
