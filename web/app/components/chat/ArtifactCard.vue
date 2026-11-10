<script setup lang="ts">
// A file the model showed, in the turn that showed it: a small card naming
// it and its version. Click → the artifact panel at this version.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { Node } from '../../lib/types'
import { showArtifact, versionOf } from '../../lib/artifacts'
import { state } from '../../lib/store'
import { workspaceLabel } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ node: Node }>()

const shown = computed(() => versionOf(props.node.id))
const open = computed(() => state.artifact === props.node.id)

/** `@scratch:page.html` → `scratch · page.html`, the way the composer names it. */
const where = computed(() => {
  const path = shown.value?.artifact.path ?? ''
  const i = path.indexOf(':')
  return i < 0 ? path : `${workspaceLabel(path.slice(0, i))} · ${path.slice(i + 1)}`
})

const ICONS = { html: 'artifact', markdown: 'note', svg: 'image', code: 'file' } as const
</script>

<template>
  <button
    v-if="shown"
    type="button"
    :data-path-node="node.id"
    class="artifact-card"
    :class="{ 'is-open': open }"
    :title="t('chat.artifact.open', { title: shown.artifact.title })"
    @click="showArtifact(open ? null : node.id)"
  >
    <span class="artifact-card-icon">
      <Icon :name="ICONS[shown.artifact.kind]" :size="16" />
    </span>
    <span class="flex min-w-0 flex-col items-start">
      <span class="max-w-full truncate font-semibold text-ink">{{ shown.artifact.title }}</span>
      <span class="max-w-full truncate font-mono text-micro text-ink-3">{{ where }}</span>
    </span>
    <span v-if="shown.version > 1" class="ml-auto flex-none font-mono text-micro text-ink-3">
      {{ t('chat.artifact.version', { n: shown.version }) }}
    </span>
  </button>
</template>

<style scoped>
.artifact-card {
  display: flex;
  width: min(100%, 480px);
  align-items: center;
  gap: 10px;
  padding: 8px 12px 8px 8px;
  border: 1px solid var(--line);
  border-radius: var(--radius-control);
  background: var(--surface);
  box-shadow: var(--shadow-soft-v);
  font-size: var(--text-meta);
  text-align: left;
  transition:
    border-color 0.15s,
    box-shadow 0.15s;
}
.artifact-card:hover {
  border-color: color-mix(in srgb, var(--tool) 45%, var(--line));
}
.artifact-card.is-open {
  border-color: var(--tool);
  box-shadow: 0 0 0 1px var(--tool);
}
.artifact-card-icon {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 32px;
  border-radius: calc(var(--radius-control) - 2px);
  background: var(--tool-soft);
  color: var(--tool);
  line-height: 0;
}
</style>
