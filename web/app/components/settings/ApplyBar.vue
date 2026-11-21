<script setup lang="ts">
// The foot of a settings section whose changes wait to be applied: Discard
// (while there is a change) and Apply, like the forms' Save.
import { t } from '../../i18n'

defineProps<{ changed: boolean; saving: boolean }>()
const emit = defineEmits<{ discard: []; apply: [] }>()
</script>

<template>
  <footer class="flex flex-none items-center justify-end gap-2 border-t border-line-soft px-4 py-3 sm:px-6">
    <button v-if="changed" type="button" class="btn" :disabled="saving" @click="emit('discard')">
      {{ t('settings.discard') }}
    </button>
    <button type="button" class="btn btn-primary" :disabled="!changed || saving" @click="emit('apply')">
      {{ t('settings.apply') }}
    </button>
  </footer>
</template>
