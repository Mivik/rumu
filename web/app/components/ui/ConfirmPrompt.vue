<script setup lang="ts">
// The confirm step of a small destructive action, in place inside a menu or
// popup: a question, an optional warning, and Cancel / the action as quiet
// text buttons (the session menu's delete, stopping background work).
import { t } from '../../i18n'

defineProps<{
  prompt: string
  warning?: string
  /** The action's label, in danger red. */
  confirm: string
}>()

const emit = defineEmits<{ cancel: []; confirm: [] }>()
</script>

<template>
  <div class="px-2.5 py-2">
    <p class="text-secondary text-ink-2">{{ prompt }}</p>
    <p v-if="warning" class="mt-1 text-micro text-ink-3">{{ warning }}</p>
    <div class="mt-2 flex items-center justify-end gap-3">
      <button
        type="button"
        class="text-micro font-semibold text-ink-2 transition-colors hover:text-ink"
        @click="emit('cancel')"
      >
        {{ t('common.cancel') }}
      </button>
      <button
        type="button"
        class="text-micro font-semibold text-danger transition-colors hover:underline"
        @click="emit('confirm')"
      >
        {{ confirm }}
      </button>
    </div>
  </div>
</template>
