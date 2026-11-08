<script setup lang="ts">
// Where models are picked before any provider is set up: the link opens
// Settings at adding one. `compact` (the composer's picker) drops the cat.
import { t } from '../../i18n'
import { addProvider } from '../../lib/layout'
import RumuCat from '../brand/RumuCat.vue'

defineProps<{ compact?: boolean }>()
const emit = defineEmits<{ add: [] }>()

function add(): void {
  addProvider()
  emit('add')
}
</script>

<template>
  <div
    class="flex flex-col items-center justify-center gap-3 text-center"
    :class="compact ? 'px-4 py-5' : 'flex-1 px-6 pb-10'"
  >
    <RumuCat v-if="!compact" state="waiting" :size="56" :paper="false" class="opacity-90" />
    <i18n-t keypath="settings.noProviders" tag="p" scope="global" class="text-secondary text-ink-3 italic">
      <template #link>
        <button
          type="button"
          class="text-ink-2 italic underline decoration-line underline-offset-4 transition-colors hover:text-brand hover:decoration-brand"
          @click="add"
        >
          {{ t('settings.addProviderLink') }}
        </button>
      </template>
    </i18n-t>
  </div>
</template>
