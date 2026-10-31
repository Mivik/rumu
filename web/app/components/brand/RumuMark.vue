<script setup lang="ts">
// The wordmark: r plus the umu line. The coral dot is the m's middle stem,
// not a full stop, so it sits mid-word. It breathes while a turn runs and
// becomes a hollow ring when the socket drops.
withDefaults(
  defineProps<{
    height?: number
    /** Breathe the dot (a turn is running). */
    live?: boolean
    /** Draw the dot as a ring (offline). */
    offline?: boolean
    /** One-shot entrance: the line writes itself, then the dot lands. */
    draw?: boolean
  }>(),
  { height: 22, live: false, offline: false, draw: false },
)

const R = 'M8.4 46.3 V37 A7.8 7.8 0 0 1 16.2 29.2 H18'
const UMU =
  'M21.5 38.5 A7.8 7.8 0 0 0 37.1 38.5 V35.6 A6.4 6.4 0 0 1 49.9 35.6 A6.4 6.4 0 0 1 62.7 35.6 V38.5 A7.8 7.8 0 0 0 78.3 38.5'
</script>

<template>
  <svg
    :height="height"
    :width="(height * 86) / 34"
    viewBox="0 20 86 34"
    role="img"
    aria-label="rumu"
    class="block overflow-visible"
  >
    <g fill="none" stroke="currentColor" stroke-width="7.4" stroke-linecap="round" stroke-linejoin="round">
      <path :d="R" pathLength="1" :class="draw ? 'mark-draw' : undefined" />
      <path :d="UMU" pathLength="1" :class="draw ? 'mark-draw mark-draw-2' : undefined" />
    </g>
    <circle
      cx="49.9"
      cy="45.5"
      r="4.6"
      :fill="offline ? 'none' : 'var(--brand)'"
      :stroke="offline ? 'var(--brand)' : undefined"
      :stroke-width="offline ? 2.2 : undefined"
      :class="[live && !offline ? 'mark-live' : '', draw ? 'mark-drop' : '']"
    />
  </svg>
</template>

<style scoped>
/* The gap is wider than the dash so the hidden start state leaves no dash
 * touching the path: a 1:1 pattern starts a dash exactly at the path's end,
 * and the round cap renders that as a stray dot that pops out before the
 * stroke begins drawing from the other end. */
.mark-draw {
  stroke-dasharray: 1 2;
  animation: mark-draw 0.55s cubic-bezier(0.5, 0, 0.3, 1) both;
}

.mark-draw-2 {
  animation-delay: 0.06s;
}

/* The dot wells up in place as the stroke reaches it — it sits midway along
 * the UMU line, where the draw arrives at ~0.29 s. Growing from the centre
 * keeps it from dropping in from above ahead of the line. */
.mark-drop {
  transform-box: fill-box;
  transform-origin: center;
  animation: mark-drop 0.4s cubic-bezier(0.34, 1.4, 0.64, 1) 0.27s both;
}

.mark-live {
  animation: mark-breathe 2.4s ease-in-out infinite;
  transform-box: fill-box;
  transform-origin: center;
}

@keyframes mark-draw {
  from {
    stroke-dashoffset: 1.01;
  }
  to {
    stroke-dashoffset: 0;
  }
}

@keyframes mark-drop {
  from {
    opacity: 0;
    transform: scale(0.3);
  }
  40% {
    opacity: 1;
  }
  to {
    opacity: 1;
    transform: scale(1);
  }
}

@keyframes mark-breathe {
  0%,
  100% {
    opacity: 1;
    transform: scale(1);
  }
  50% {
    opacity: 0.55;
    transform: scale(0.82);
  }
}
</style>
