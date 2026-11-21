<script setup lang="ts">
// An on/off switch: a pill whose knob slides over when on.
defineProps<{ label: string; disabled?: boolean }>()
const model = defineModel<boolean>({ required: true })
</script>

<template>
  <button
    type="button"
    role="switch"
    :aria-checked="model"
    :aria-label="label"
    :disabled="disabled"
    class="relative inline-flex h-5 w-9 flex-none rounded-full transition-colors duration-150"
    :class="model ? 'bg-ink' : 'bg-line'"
    @click="model = !model"
  >
    <span
      class="absolute top-0.5 left-0.5 size-4 rounded-full bg-surface shadow-soft transition-transform duration-150 ease-soft"
      :class="{ 'translate-x-4': model }"
    />
  </button>
</template>
