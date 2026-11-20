<script setup lang="ts">
// A project on the projects grid: a band of soft color drawn from its id
// (each project its own blend, so two of a tint still tell apart) with its
// seal on the edge, then its name, its workspaces and a new session in it.
// The card itself opens the project.
import { computed } from 'vue'
import { locale, t } from '../../i18n'
import { projectTone, random, toneColor } from '../../lib/projectIcon'
import { startSessionIn, state } from '../../lib/store'
import type { Project } from '../../lib/types'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ project: Project }>()
defineEmits<{ open: [] }>()

/** Three soft lights — the tone and its two hue neighbours — placed by the
 * id; `--k` scales how much tone they carry (less in the dark). */
const mesh = computed(() => {
  const rnd = random(props.project.id)
  const d = Math.round(25 + rnd() * 25)
  const turn = Math.floor(rnd() * 3)
  return [60, 45, 30]
    .map((mix, k) => {
      const dh = [-d, 0, d][(k + turn) % 3]
      const color = `oklch(from var(--tone) l c calc(h + ${dh}))`
      const at = `${Math.round(rnd() * 100)}% ${Math.round(rnd() * 100)}%`
      return `radial-gradient(circle at ${at}, color-mix(in oklab, ${color} calc(var(--k) * ${mix}%), var(--surface)), transparent 65%)`
    })
    .join(', ')
})

/** Its workspaces, unless the one it mounts is just its own name again. */
const workspaces = computed(() => {
  const ws = props.project.workspaces
  return ws.length === 1 && ws[0] === props.project.name ? [] : ws
})

/** When a session in it last moved, as a relative time. */
const lastActive = computed(() => {
  let at = 0
  for (const s of state.sessions) if (s.project === props.project.id) at = Math.max(at, s.updated_at)
  if (!at) return ''
  const fmt = new Intl.RelativeTimeFormat(locale.value, { numeric: 'auto' })
  const mins = Math.round((at - Date.now()) / 60_000)
  if (mins > -60) return fmt.format(mins, 'minute')
  if (mins > -60 * 24) return fmt.format(Math.round(mins / 60), 'hour')
  return fmt.format(Math.round(mins / 1440), 'day')
})
</script>

<template>
  <div class="card" :style="{ '--tone': toneColor(projectTone(project)) }">
    <button type="button" class="open" :aria-label="project.name" @click="$emit('open')" />
    <div class="band" :style="{ '--mesh': mesh }" />
    <ProjectSeal :project="project" :size="40" class="seal" />
    <div class="pointer-events-none relative flex min-w-0 flex-1 flex-col gap-1.5 px-4 pt-2 pb-3">
      <span class="truncate text-message font-semibold text-ink">{{ project.name }}</span>
      <span v-if="workspaces.length || !project.workspaces.length" class="flex min-w-0 flex-wrap gap-1">
        <span
          v-for="w in workspaces"
          :key="w"
          class="max-w-full truncate rounded-xs bg-surface-2 px-1.5 font-card text-micro text-ink-2"
          >{{ w }}</span
        >
        <span v-if="!project.workspaces.length" class="text-micro text-ink-3">{{
          t('projects.noWorkspaces')
        }}</span>
      </span>
      <span class="mt-auto flex items-center gap-2 pt-1">
        <span class="min-w-0 flex-1 truncate text-micro text-ink-3">{{ lastActive }}</span>
        <button type="button" class="start" @click="startSessionIn(project.id)">
          <Icon name="plus" :size="13" />
          {{ t('projects.newSession') }}
        </button>
      </span>
    </div>
  </div>
</template>

<style scoped>
.card {
  position: relative;
  display: flex;
  min-width: 0;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--line-soft);
  border-radius: var(--radius-card);
  background: var(--surface);
  box-shadow: var(--shadow-soft-v);
  transition:
    box-shadow 0.25s var(--ease-soft),
    translate 0.25s var(--ease-soft),
    border-color 0.25s var(--ease-soft);
}

.card:hover,
.card:has(.open:focus-visible) {
  translate: 0 -2px;
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
  outline: none;
}

.start {
  pointer-events: auto;
  position: relative;
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

.band {
  --k: 1;
  height: 4.75rem;
  background:
    var(--grain), var(--mesh), color-mix(in oklab, var(--tone) calc(var(--k) * 22%), var(--surface));
}

.seal {
  pointer-events: none;
  position: relative;
  margin: -1.25rem 0 0 1rem;
  border-radius: 12px !important;
  box-shadow: 0 0 0 3px var(--surface);
}

:global(html[data-theme='dark']) .band {
  --k: 0.65;
}
</style>
