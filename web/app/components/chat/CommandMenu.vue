<script setup lang="ts">
// The slash commands a draft of `/` and a few letters could mean, floating
// above the composer (over the conversation, like SelectionBar, so the
// writing never moves). The composer keeps the keys: ↑↓ pick, Tab or Enter
// complete, Esc puts it away; a press picks without taking the caret.
import type { Command } from '../../lib/state/commands'
import { t } from '../../i18n'
import Icon from '../ui/Icon.vue'

defineProps<{ items: Command[]; active: number; query: string }>()
const emit = defineEmits<{ pick: [Command]; hover: [number] }>()
</script>

<template>
  <div
    class="absolute bottom-full left-0 z-20 mb-2 w-[min(100%,24rem)] origin-bottom-left rounded-card border border-line bg-surface p-1 shadow-pop"
    role="listbox"
    :aria-label="t('chat.command.menu')"
    @pointerdown.prevent
  >
    <button
      v-for="(c, i) in items"
      :key="c.name"
      type="button"
      role="option"
      :aria-selected="i === active"
      class="cmd-row"
      :class="[i === active && 'is-active', c.tone && `tone-${c.tone}`]"
      @mousemove="i !== active && emit('hover', i)"
      @click="emit('pick', c)"
    >
      <span class="cmd-icon"><Icon :name="c.icon" :size="14" /></span>
      <span class="min-w-0 flex-none font-mono text-[13px]">
        <span class="cmd-typed">/{{ c.name.slice(0, query.length) }}</span
        >{{ c.name.slice(query.length) }}
        <span v-if="c.arg" class="text-ink-3">‹{{ c.arg }}›</span>
      </span>
      <span class="min-w-0 flex-1 truncate text-right text-meta text-ink-3">{{ c.label }}</span>
    </button>
  </div>
</template>

<style scoped>
.cmd-row {
  --tone: var(--ink-2);
  display: flex;
  width: 100%;
  min-width: 0;
  align-items: center;
  gap: 0.625rem;
  border-radius: var(--radius-control);
  padding: 0.3125rem 0.625rem 0.3125rem 0.375rem;
  color: var(--ink);
  text-align: left;
}
.cmd-row.tone-brand {
  --tone: var(--brand);
}
.cmd-row.tone-agent {
  --tone: var(--agent);
}
.cmd-row.is-active {
  background: color-mix(in srgb, var(--ink) 7%, transparent);
}

.cmd-icon {
  display: inline-flex;
  flex: none;
  width: 1.5rem;
  height: 1.5rem;
  align-items: center;
  justify-content: center;
  border-radius: 0.4375rem;
  background: color-mix(in srgb, var(--tone) 10%, transparent);
  color: var(--tone);
  transition: background-color 0.15s;
}
.is-active .cmd-icon {
  background: color-mix(in srgb, var(--tone) 18%, transparent);
}

.cmd-typed {
  color: var(--tone);
  font-weight: 600;
}
</style>
