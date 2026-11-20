<script setup lang="ts">
// The way back to the sub-agent a task output or artifact replaced beside
// the chat (opened from its panel, or over it).
import { computed } from 'vue'
import { t } from '../../i18n'
import { openAgentView, state } from '../../lib/store'
import Icon from '../ui/Icon.vue'

const back = computed(() => (state.panelBack && state.branches[state.panelBack] ? state.panelBack : null))
</script>

<template>
  <button
    v-if="back"
    type="button"
    class="icon-btn size-7 flex-none"
    :title="t('chat.agentView.backTo')"
    :aria-label="t('chat.agentView.backTo')"
    @click="openAgentView(back)"
  >
    <Icon name="back" :size="14" />
  </button>
</template>
