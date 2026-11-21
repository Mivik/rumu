<script setup lang="ts">
// One setting: its name on the left, its control on the right — under the
// name when the pane (an `@container`) is too narrow for both. Rows stack in
// a SettingGroup, hairlines between them.
withDefaults(defineProps<{ label: string; wide?: boolean; disabled?: boolean }>(), {
  wide: false,
  disabled: false,
})
</script>

<template>
  <div
    class="flex min-h-12 flex-wrap items-center justify-between gap-x-4 gap-y-1.5 py-2 transition-opacity"
    :class="{ 'pointer-events-none opacity-50': disabled }"
  >
    <span class="flex min-w-0 items-center gap-1.5 text-ink">
      {{ label }}
      <slot name="label" />
    </span>
    <div class="flex w-full justify-end" :class="wide ? '@min-[20rem]:w-56' : '@min-[20rem]:w-44'">
      <slot />
    </div>
  </div>
</template>
