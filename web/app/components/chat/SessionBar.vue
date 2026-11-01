<script setup lang="ts">
// The open session's diagnostics (cache meters, hook trace) in the chat
// header's end. It opens a popover; nothing about the session takes a
// column of its own.
import { computed } from 'vue'
import { t } from '../../i18n'
import { cacheTotal } from '../../lib/store'
import CachePanel from '../CachePanel.vue'
import HookTracePanel from '../HookTracePanel.vue'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const cachePct = computed(() => {
  const c = cacheTotal.value
  return c.prompt > 0 ? Math.round((c.cached * 100) / c.prompt) : null
})
</script>

<template>
  <Popover :label="t('panels.diagnostics.title')" wide>
    <template #trigger="{ open, toggle }">
      <button
        type="button"
        class="bar-btn"
        :class="{ 'is-open': open }"
        :title="t('panels.diagnostics.title')"
        :aria-label="t('panels.diagnostics.title')"
        :aria-expanded="open"
        @click="toggle"
      >
        <Icon name="gauge" :size="15" class="text-ink-3" />
        <span v-if="cachePct !== null" class="hidden tabular-nums sm:inline">{{
          t('panels.diagnostics.cache', { pct: cachePct })
        }}</span>
      </button>
    </template>
    <div class="panel-head">{{ t('panels.diagnostics.title') }}</div>
    <div class="flex flex-col gap-4">
      <CachePanel />
      <HookTracePanel />
    </div>
  </Popover>
</template>
