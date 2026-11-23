<script lang="ts">
export type { CatPose, CatState } from '../../lib/cat'
</script>

<script setup lang="ts">
// The mascot, driven parametrically (lib/cat.ts): each state is a pose, and a
// state change morphs from whatever is on screen to the new pose. Waiting,
// thinking and working are the loops (see poseAt), and they stop when the
// turn does. `cat` gives it a build and things to wear (lib/catItems).
import { computed, onBeforeUnmount, onMounted, shallowRef, watch } from 'vue'
import { catBuild } from '../../lib/assistantFace'
import {
  eyesD,
  eyeWidth,
  faceD,
  featuresTransform,
  isAnimated,
  lerpPose,
  lineD,
  mouthD,
  poseAt,
  type CatPose,
  type CatState,
} from '../../lib/cat'
import { drawItems, type CatPrim } from '../../lib/catItems'
import { prefersReduced } from '../../lib/motion'
import { toneColor, type ProjectTone } from '../../lib/projectIcon'
import type { DeckCat } from '../../lib/types'

const props = withDefaults(
  defineProps<{
    state?: CatState
    /** Overrides on top of the state's pose, e.g. `{ look: 1 }`. Changes morph too. */
    pose?: Partial<CatPose>
    /** Width in px; the drawing is 5:4. */
    size?: number
    /** The paper face is for large sizes; line-only below ~64px. */
    paper?: boolean
    /** Its build and what it wears; changes morph too. */
    cat?: DeckCat
  }>(),
  { state: 'idle', size: 96, paper: true },
)

/** The morph between poses; inside the 200–450 ms band, eased like ease-soft. */
const MORPH = 380
const easeSoft = (t: number): number => 1 - (1 - t) ** 4

const target = (t: number): CatPose => ({
  ...poseAt(props.state, t),
  ...(props.cat && catBuild(props.cat)),
  ...props.pose,
})

const shown = shallowRef<CatPose>(target(0))

/** The loops are slow sways: drawn at this pace (ms a frame), not every
 *  display frame — they run as long as a turn does. Morphs stay smooth. */
const LOOP_FRAME = 50

let reduced = false
let raf = 0
let timer = 0 // the next loop frame, waiting out LOOP_FRAME
let since = 0 // when the current state began
let from: CatPose | null = null // the pose the morph started at
let fromAt = 0

function frame(now: number): void {
  raf = 0
  const goal = target(now - since)
  const p = from ? Math.min((now - fromAt) / MORPH, 1) : 1
  shown.value = from && p < 1 ? lerpPose(from, goal, easeSoft(p)) : goal
  if (p >= 1) from = null
  if (from) raf = requestAnimationFrame(frame)
  else if (isAnimated(props.state)) timer = window.setTimeout(next, LOOP_FRAME)
}

function next(): void {
  timer = 0
  raf = requestAnimationFrame(frame)
}

/** Draw from the next frame on, unless a frame is already due sooner. */
function start(): void {
  if (raf) return
  clearTimeout(timer)
  timer = 0
  raf = requestAnimationFrame(frame)
}

function stop(): void {
  cancelAnimationFrame(raf)
  clearTimeout(timer)
  raf = 0
  timer = 0
  from = null
}

function morph(restart: boolean): void {
  if (reduced) {
    shown.value = target(0)
    return
  }
  const now = performance.now()
  if (restart) since = now
  from = shown.value
  fromAt = now
  start()
}

watch(
  () => props.state,
  () => morph(true),
)
watch([() => props.pose, () => props.cat?.wide, () => props.cat?.ears], () => morph(false), { deep: true })

const items = computed(() =>
  drawItems(shown.value, props.cat?.items ?? [], toneColor((props.cat?.tone ?? 'brand') as ProjectTone)),
)

let mq: MediaQueryList | null = null
const sync = (): void => {
  reduced = prefersReduced()
  if (reduced) {
    stop()
    shown.value = target(0)
  } else if (isAnimated(props.state) && !raf && !timer) {
    since = performance.now()
    start()
  }
}
onMounted(() => {
  mq = window.matchMedia('(prefers-reduced-motion: reduce)')
  sync()
  mq.addEventListener('change', sync)
})
onBeforeUnmount(() => {
  stop()
  mq?.removeEventListener('change', sync)
})

const ROUND = { fill: 'none', 'stroke-linecap': 'round', 'stroke-linejoin': 'round' } as const

const prim = (s: CatPrim) => ({
  ...ROUND,
  d: s.d,
  fill: s.fill ?? 'none',
  stroke: s.stroke,
  'stroke-width': s.width,
  transform: s.transform,
  class: s.class,
})
</script>

<template>
  <svg
    :width="size"
    :height="size * 0.8"
    viewBox="10 14 80 64"
    aria-hidden="true"
    class="block overflow-visible"
  >
    <path v-for="(s, i) in items.body" :key="`b${i}`" v-bind="prim(s)" />
    <path v-if="paper" class="cat-face" :d="faceD(shown)" fill="var(--surface)" />

    <path v-bind="ROUND" :d="lineD(shown)" stroke="var(--ink)" stroke-width="5.5" />

    <g :transform="featuresTransform(shown)">
      <path v-bind="ROUND" :d="eyesD(shown)" stroke="var(--ink)" :stroke-width="eyeWidth(shown)" />
      <path v-for="(s, i) in items.face" :key="`f${i}`" v-bind="prim(s)" />
      <path
        v-bind="ROUND"
        :d="mouthD(shown)"
        :style="{
          stroke: `color-mix(in oklab, var(--ink-3) ${(shown.mouthGrey * 100).toFixed(1)}%, var(--brand))`,
        }"
        stroke-width="2.5"
      />
    </g>

    <path v-for="(s, i) in items.head" :key="`h${i}`" v-bind="prim(s)" />

    <!-- the stamp lands: it shrinks onto the paper as it inks in -->
    <g
      v-if="shown.stamp > 0.01"
      :transform="`rotate(-14 76 62) translate(76 62) scale(${1.3 - 0.3 * shown.stamp}) translate(-76 -62)`"
      :opacity="0.85 * shown.stamp"
    >
      <rect
        x="65"
        y="55.5"
        width="22"
        height="13"
        rx="3"
        fill="none"
        stroke="var(--danger)"
        stroke-width="2.4"
      />
      <path d="M70 62 H82" stroke="var(--danger)" stroke-width="2.4" stroke-linecap="round" />
    </g>
  </svg>
</template>

<style scoped>
/* The paper face only shows in dark mode; on cream the line alone is enough. */
.cat-face {
  display: none;
}

.cat-blush {
  opacity: 0.32;
}

html[data-theme='dark'] .cat-blush {
  fill: color-mix(in oklab, var(--brand) 55%, white);
  opacity: 0.4;
}

html[data-theme='dark'] .cat-face {
  display: inline;
  filter: drop-shadow(0 2px 3px rgb(0 0 0 / 40%));
}
</style>
