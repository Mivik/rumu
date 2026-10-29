<script setup lang="ts">
// Minimal stroke icon set (one visual weight, inherits currentColor).
import { computed } from 'vue'

const props = withDefaults(defineProps<{ name: string; size?: number; stroke?: number }>(), {
  size: 14,
  stroke: 1.8,
})

const paths: Record<string, string> = {
  user: '<circle cx="12" cy="8" r="3.6"/><path d="M4.5 20.5c.6-4 3.7-6 7.5-6s6.9 2 7.5 6"/>',
  memory:
    '<path d="M12 3.5l8.5 4.5L12 12.5 3.5 8 12 3.5z"/><path d="M3.5 12.5L12 17l8.5-4.5"/><path d="M3.5 16.5L12 21l8.5-4.5"/>',
  tag: '<path d="M4 4h7.2l8.8 8.8-7.2 7.2L4 11.2V4z"/><circle cx="8.7" cy="8.7" r="1.3"/>',
  mount:
    '<path d="M3.5 7.5a2 2 0 0 1 2-2h4l2 2.2h7a2 2 0 0 1 2 2v8.8a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2v-11z"/>',
  tool: '<rect x="3.5" y="5" width="17" height="14" rx="2"/><path d="M7.5 10l3 2.5-3 2.5"/><path d="M12.5 15h4"/>',
  regenerate: '<path d="M20 12a8 8 0 1 1-2.34-5.66"/><path d="M20 4v5h-5"/>',
  copy: '<rect x="9" y="9" width="10.5" height="10.5" rx="2"/><path d="M15 9V6.5A2.5 2.5 0 0 0 12.5 4h-6A2.5 2.5 0 0 0 4 6.5v6A2.5 2.5 0 0 0 6.5 15H9"/>',
  edit: '<path d="M15.5 4.5l4 4L8 20H4v-4L15.5 4.5z"/><path d="M13 7l4 4"/>',
  agent: '<circle cx="12" cy="12" r="8.5"/><path d="M15 9l-2 4-4 2 2-4 4-2z"/>',
  fork: '<circle cx="6.5" cy="5.5" r="2.2"/><circle cx="6.5" cy="18.5" r="2.2"/><circle cx="17.5" cy="5.5" r="2.2"/><path d="M6.5 7.7v8.6"/><path d="M17.5 7.7v1.8a5 5 0 0 1-5 5H9"/>',
  blocked: '<circle cx="12" cy="12" r="8.5"/><path d="M6.5 6.5l11 11"/>',
  error: '<circle cx="12" cy="12" r="8.5"/><path d="M9 9l6 6M15 9l-6 6"/>',
  ok: '<path d="M5.5 13l4.2 4.2L18.5 8"/>',
  note: '<path d="M5 4.5A1.5 1.5 0 0 1 6.5 3h11A1.5 1.5 0 0 1 19 4.5v15a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 5 19.5v-15z"/><path d="M8.5 8h7M8.5 12h7M8.5 16h4"/>',
  file: '<path d="M6 3.5h7.5l4.5 4.5v12.5H6v-17z"/><path d="M13.5 3.5V8H18"/>',
  external: '<path d="M7 17L17 7"/><path d="M9.5 7H17v7.5"/>',
  close: '<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>',
  menu: '<path d="M4 7h16M4 12h16M4 17h16"/>',
  panel: '<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><path d="M15 4.5v15"/>',
  chevron: '<path d="M9 6l6 6-6 6"/>',
  down: '<path d="M6.5 9.5l5.5 5.5 5.5-5.5"/>',
  gauge:
    '<path d="M4.2 16.5a8 8 0 1 1 15.6 0"/><path d="M12 15l3.6-4.2"/><circle cx="12" cy="15.5" r="1.2"/>',
  plus: '<path d="M12 5.5v13M5.5 12h13"/>',
  send: '<path d="M12 19V5.5"/><path d="M6.5 11L12 5.5l5.5 5.5"/>',
  stop: '<rect x="7" y="7" width="10" height="10" rx="2" fill="currentColor" stroke="none"/>',
  brain:
    '<path d="M9.5 4.5A3 3 0 0 0 6.6 8 3 3 0 0 0 5 10.8a3 3 0 0 0 1.5 2.6A3 3 0 0 0 9.8 19a2.8 2.8 0 0 0 2.2-1V5.3a2.6 2.6 0 0 0-2.5-.8z"/><path d="M14.5 4.5A3 3 0 0 1 17.4 8 3 3 0 0 1 19 10.8a3 3 0 0 1-1.5 2.6A3 3 0 0 1 14.2 19a2.8 2.8 0 0 1-2.2-1V5.3a2.6 2.6 0 0 1 2.5-.8z"/>',
}

const html = computed(() => paths[props.name] ?? paths.note)
</script>

<template>
  <svg
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    :stroke-width="stroke"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    class="block flex-none"
    v-html="html"
  />
</template>
