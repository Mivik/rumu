<script setup lang="ts">
// A project on the projects grid: a band of contour lines drawn from its id
// (each project its own terrain, so two of a tint still tell apart) with its
// seal on the edge, then its name, its workspaces and a new session in it.
// The card itself opens the project.
import { computed } from 'vue'
import { locale, t } from '../../i18n'
import { hash, projectTone, toneColor } from '../../lib/projectIcon'
import { startSessionIn, state } from '../../lib/store'
import type { Project } from '../../lib/types'
import ProjectSeal from '../brand/ProjectSeal.vue'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ project: Project }>()
defineEmits<{ open: [] }>()

const W = 240
const H = 80

/** A small seeded PRNG (mulberry32) over the id's hash. */
function random(seed: string): () => number {
  let h = hash(seed)
  return () => {
    h = (h + 0x6d2b79f5) | 0
    let x = Math.imul(h ^ (h >>> 15), h | 1)
    x ^= x + Math.imul(x ^ (x >>> 7), x | 61)
    return ((x ^ (x >>> 14)) >>> 0) / 4294967296
  }
}

const rings = computed(() => {
  const rnd = random(props.project.id)
  // a hill somewhere on the band, its rings widening out past the edges
  const cx = W * (0.35 + rnd() * 0.55)
  const cy = H * (0.2 + rnd() * 0.6)
  const k1 = 2 + Math.floor(rnd() * 3)
  const k2 = 3 + Math.floor(rnd() * 4)
  const p1 = rnd() * Math.PI * 2
  const p2 = rnd() * Math.PI * 2
  const rings: string[] = []
  let r = 4 + rnd() * 6
  for (let i = 1; i <= 11; i++) {
    // uneven steps: slopes steepen and flatten like real ground
    r += 8 + rnd() * 10
    const wobble = 0.05 + 0.1 * (i / 11)
    let d = ''
    for (let k = 0; k <= 72; k++) {
      const a = (k / 72) * Math.PI * 2
      const f = 1 + wobble * (0.7 * Math.sin(k1 * a + p1) + 0.3 * Math.sin(k2 * a + p2 + i * 0.3))
      d += `${k ? 'L' : 'M'}${(cx + Math.cos(a) * r * f * 1.35).toFixed(1)} ${(cy + Math.sin(a) * r * f).toFixed(1)}`
    }
    rings.push(`${d}Z`)
  }
  return rings
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
    <svg class="band" :viewBox="`0 0 ${W} ${H}`" preserveAspectRatio="xMidYMid slice" aria-hidden="true">
      <path v-for="(d, i) in rings" :key="i" :d="d" :style="{ opacity: 0.6 - i * 0.04 }" />
    </svg>
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
  pointer-events: none;
  display: block;
  width: 100%;
  height: 4.75rem;
  background:
    var(--grain),
    linear-gradient(
      120deg,
      color-mix(in oklab, var(--tone) 22%, var(--surface)),
      color-mix(in oklab, var(--tone) 58%, var(--surface))
    );
}

.band path {
  fill: none;
  stroke: color-mix(in oklab, var(--tone) 55%, var(--ink));
  stroke-width: 1;
  vector-effect: non-scaling-stroke;
  transition: translate 0.8s var(--ease-soft);
}

/* the terrain shifts a little under the pointer */
.card:hover .band path:nth-child(odd) {
  translate: -2px 1px;
}

.card:hover .band path:nth-child(even) {
  translate: 2px -1px;
}

.seal {
  pointer-events: none;
  position: relative;
  margin: -1.25rem 0 0 1rem;
  border-radius: 12px !important;
  box-shadow: 0 0 0 3px var(--surface);
}

@media (prefers-color-scheme: dark) {
  .band {
    background:
      var(--grain),
      linear-gradient(
        120deg,
        color-mix(in oklab, var(--tone) 8%, var(--surface)),
        color-mix(in oklab, var(--tone) 24%, var(--surface))
      );
  }

  .band path {
    stroke: color-mix(in oklab, var(--tone) 80%, var(--surface));
  }
}

@media (prefers-reduced-motion: reduce) {
  .card:hover .band path {
    translate: none;
  }
}
</style>
