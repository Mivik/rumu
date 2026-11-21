<script setup lang="ts">
// A titled group of SettingRows, hairlines between them.
defineProps<{ title?: string }>()
</script>

<template>
  <section class="flex flex-col">
    <h3 v-if="title" class="field-label">{{ title }}</h3>
    <div class="flex flex-col divide-y divide-line-soft"><slot /></div>
  </section>
</template>
