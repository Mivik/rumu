<script setup lang="ts">
// An assistant's avatar: its own cat (lib/assistantFace) centred on a tile of
// its tone. Without an id (one not yet created) the tile is plain and the cat
// at rest. `near` turns the cat to face whoever points at it.
import { computed } from 'vue'
import { assistantFace, assistantTone, attentive } from '../../lib/assistantFace'
import RumuCat from './RumuCat.vue'

const props = withDefaults(defineProps<{ id?: string; size?: number; near?: boolean }>(), { size: 48 })

const face = computed(() => (props.id ? assistantFace(props.id) : {}))
const pose = computed(() => (props.near ? attentive(face.value) : face.value))
</script>

<template>
  <span
    class="avatar"
    :style="{
      '--tone': id ? assistantTone(id) : 'var(--ink-3)',
      width: `${size}px`,
      height: `${size}px`,
      borderRadius: `${Math.round(size * 0.3)}px`,
    }"
    aria-hidden="true"
  >
    <RumuCat :size="Math.round(size * 0.88)" :paper="false" :pose="pose" />
  </span>
</template>

<style scoped>
.avatar {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  border: 1px solid color-mix(in oklab, var(--tone) 28%, var(--line-soft));
  background: color-mix(in oklab, var(--tone) 12%, var(--surface));
  box-shadow: var(--shadow-soft-v);
}
</style>
