<script setup lang="ts">
// An assistant on the assistants grid: a band of its tone with its avatar on
// the edge, then its name, the start of its instructions, what it carries
// (mounted decks, MCP servers, skills) and a new session with it. The card
// itself opens the assistant. The cat turns to face whoever points at the
// card.
import { computed, ref } from 'vue'
import { t } from '../../i18n'
import { assistantTone } from '../../lib/assistantFace'
import { chooseLandingAssistant, goHome } from '../../lib/store'
import type { Assistant } from '../../lib/types'
import AssistantAvatar from '../brand/AssistantAvatar.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ assistant: Assistant }>()
defineEmits<{ open: [] }>()

const near = ref(false)

/** What it carries, each only when it has any. */
const carries = computed(() => {
  const a = props.assistant
  const servers = Object.keys(a.mcp).length
  const skills = a.skills.includes('*') ? null : a.skills.filter((r) => !r.startsWith('!')).length
  return [
    a.mounts.length && {
      icon: 'memory',
      n: a.mounts.length,
      title: t('assistants.carriesDecks', { n: a.mounts.length }),
    },
    servers && { icon: 'mcp', n: servers, title: t('assistants.carriesMcp', { n: servers }) },
    skills !== 0 && {
      icon: 'note',
      n: skills ?? t('assistants.allTools'),
      title:
        skills === null ? t('assistants.carriesAllSkills') : t('assistants.carriesSkills', { n: skills }),
    },
  ].filter((x) => !!x)
})

function start(): void {
  chooseLandingAssistant(props.assistant.id)
  goHome()
}
</script>

<template>
  <div
    class="card"
    :style="{ '--tone': assistantTone(assistant.id) }"
    @pointerenter="near = true"
    @pointerleave="near = false"
  >
    <button
      type="button"
      class="open"
      :aria-label="assistant.name"
      @click="$emit('open')"
      @focus="near = true"
      @blur="near = false"
    />
    <div class="band">
      <span v-if="assistant.default" class="badge">{{ t('assistants.defaultBadge') }}</span>
    </div>
    <AssistantAvatar :id="assistant.id" :cat="assistant.cat" :size="52" :near="near" class="on-band" />
    <div class="pointer-events-none relative flex min-w-0 flex-1 flex-col gap-1 px-4 pt-2 pb-3">
      <span class="truncate font-card text-message font-semibold text-ink">{{ assistant.name }}</span>
      <p class="brief">{{ assistant.instructions }}</p>
      <span class="mt-auto flex items-center gap-3 pt-2">
        <span class="flex min-w-0 flex-1 items-center gap-3 text-micro text-ink-3">
          <span
            v-for="c in carries"
            :key="c.icon"
            class="flex items-center gap-1 tabular-nums"
            :title="c.title"
          >
            <Icon :name="c.icon" :size="13" />
            {{ c.n }}
          </span>
        </span>
        <button type="button" class="start" @click="start">
          <Icon name="plus" :size="13" />
          {{ t('assistants.newSession') }}
        </button>
      </span>
    </div>
  </div>
</template>

<style scoped>
.card {
  --band: 4.25rem;
  position: relative;
  display: flex;
  min-width: 0;
  min-height: 11.5rem;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--line-soft);
  border-radius: var(--radius-card);
  background: var(--surface);
  box-shadow: var(--shadow-soft-v);
  transition:
    box-shadow 0.25s var(--ease-soft),
    border-color 0.25s var(--ease-soft);
}

.card:hover,
.card:has(.open:focus-visible) {
  border-color: color-mix(in srgb, var(--tone) 35%, var(--line));
  box-shadow: var(--shadow-card-v);
}

.card:has(.open:focus-visible) {
  outline: 2px solid var(--brand);
  outline-offset: 2px;
}

/* the whole card is the open button; the new-session button sits above it */
.open {
  position: absolute;
  inset: 0;
  z-index: 1;
  outline: none;
}

.band {
  --k: 1;
  position: relative;
  height: var(--band);
  flex: none;
  border-bottom: 1px solid color-mix(in oklab, var(--tone) calc(var(--k) * 30%), var(--line-soft));
  background: var(--grain), color-mix(in oklab, var(--tone) calc(var(--k) * 20%), var(--surface));
}

/* half on the band, half on the card */
.on-band {
  pointer-events: none;
  position: relative;
  margin: -26px 0 0 1rem;
}

.badge {
  position: absolute;
  top: 0.625rem;
  right: 0.75rem;
  border-radius: 999px;
  background: color-mix(in srgb, var(--surface) 80%, transparent);
  padding: 1px 8px;
  color: var(--ink-2);
  font-size: var(--text-micro);
  font-weight: 600;
}

.brief {
  display: -webkit-box;
  overflow: hidden;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  color: var(--ink-2);
  font-size: var(--text-meta);
  line-height: 1.5;
}

.start {
  pointer-events: auto;
  position: relative;
  z-index: 2;
  display: flex;
  margin-right: -0.375rem;
  align-items: center;
  gap: 0.25rem;
  border-radius: 999px;
  padding: 0.25rem 0.625rem;
  color: var(--ink-3);
  font-size: var(--text-meta);
  font-weight: 600;
  transition:
    background-color 0.15s,
    color 0.15s;
}

.start:hover {
  background: color-mix(in srgb, var(--tone) 14%, transparent);
  color: color-mix(in srgb, var(--tone) 60%, var(--ink));
}

html[data-theme='dark'] .band {
  --k: 0.9;
}
</style>
