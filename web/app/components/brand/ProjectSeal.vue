<script setup lang="ts">
// A project's seal: its glyph on a rounded square of its tone. The one mark
// for a project wherever it shows (the grid, the sidebar, the pickers, the
// chat header).
import { computed } from 'vue'
import { projectGlyph, projectTone, toneColor } from '../../lib/projectIcon'
import type { Project } from '../../lib/types'

const props = withDefaults(defineProps<{ project: Pick<Project, 'id' | 'name' | 'icon'>; size?: number }>(), {
  size: 16,
})

const glyph = computed(() => projectGlyph(props.project))
/** An emoji brings its own colors: it sits on a tint, not a solid fill. */
const emoji = computed(() => /\p{Extended_Pictographic}/u.test(glyph.value))
</script>

<template>
  <span
    class="seal"
    :class="{ emoji }"
    :style="{
      '--tone': toneColor(projectTone(project)),
      width: `${size}px`,
      height: `${size}px`,
      borderRadius: `${Math.round(size * 0.3)}px`,
      fontSize: `${Math.round(size * (emoji ? 0.62 : 0.5))}px`,
    }"
    aria-hidden="true"
    >{{ glyph }}</span
  >
</template>

<style scoped>
.seal {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  background: var(--tone);
  color: var(--surface);
  font-family: var(--font-card);
  font-weight: 700;
  line-height: 1;
  user-select: none;
}

.seal.emoji {
  background: color-mix(in srgb, var(--tone) 22%, var(--surface));
}
</style>
