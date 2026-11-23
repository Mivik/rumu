<script setup lang="ts">
// One thing a plugin registered, mounted inside an error boundary: when it
// throws (rendering, or in a hook of its own), only this spot breaks, and it
// names the plugin and the error instead of going blank.
import { onErrorCaptured, ref, type Component } from 'vue'
import { t } from '../../i18n'

defineProps<{ plugin: string; is: Component; props?: Record<string, unknown> }>()

const error = ref<string | null>(null)

onErrorCaptured((err) => {
  console.error('plugin failed:', err)
  error.value = err instanceof Error ? err.message : String(err)
  return false
})
</script>

<template>
  <span v-if="error" class="text-meta text-danger" :title="error"
    >{{ t('common.pluginFailed', { name: plugin }) }} · {{ error }}</span
  >
  <component :is="is" v-else v-bind="props" />
</template>
