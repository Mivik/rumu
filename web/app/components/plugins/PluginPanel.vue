<script setup lang="ts">
// A plugin's panel beside the chat (over it below xl), in the sheet a
// sub-agent or an artifact opens in: its title and a close key over what the
// plugin draws, inside an error boundary.
import { computed } from 'vue'
import { t } from '../../i18n'
import { openPanelOf, openPluginPanel } from '../../lib/plugins'
import Icon from '../ui/Icon.vue'
import PluginView from './PluginView.vue'

const panel = computed(openPanelOf)
</script>

<template>
  <section v-if="panel" class="sheet flex min-h-0 min-w-0 flex-col overflow-hidden rounded-sheet bg-surface">
    <header class="flex min-h-14 flex-none items-center gap-2 border-b border-line-soft px-4">
      <Icon name="plugin" :size="16" class="flex-none text-ink-3" />
      <span class="min-w-0 flex-1 truncate text-[15px] font-bold">{{ panel.title }}</span>
      <span class="flex-none font-mono text-micro text-ink-3">{{ panel.plugin }}</span>
      <button
        class="icon-btn size-7 flex-none"
        :title="t('chat.agentView.close')"
        :aria-label="t('chat.agentView.close')"
        @click="openPluginPanel(panel.plugin, null)"
      >
        <Icon name="close" :size="15" />
      </button>
    </header>
    <div class="min-h-0 flex-1 overflow-y-auto px-4 py-3">
      <PluginView :key="`${panel.plugin}:${panel.id}`" :plugin="panel.plugin" :is="panel.component" />
    </div>
  </section>
</template>
