<script setup lang="ts">
// Where a compaction happened: from here on the model sees its summary in
// place of the older messages. The line names the size before and after; it
// opens to the summary itself.
//
// While the summary is being written (`live`), the line is drawn where it
// will land: coral pulses run along both rules into the middle, the label
// saying how big the view was and how long it has taken. A line that has
// just been written takes over from it with the pulse let go outwards and
// the size counting down to the one after.
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../../i18n'
import { compactionOf } from '../../lib/store'
import { renderMarkdown } from '../../lib/markdown'
import { vEnter } from '../../lib/motion'
import type { Node } from '../../lib/types'
import { fmtThought } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ node?: Node; live?: { since: number; before: number } }>()

/** A line written this recently is the one the live line just became. */
const FRESH_MS = 4000
const COUNT_MS = 900

const open = ref(false)
const data = computed(() => (props.node ? compactionOf(props.node) : null))

const tokens = new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 })

/** The summary alone: the note also carries the user's own messages, which
 *  the timeline shows already. */
const summary = computed(() => {
  const b = props.node?.body
  const text = b?.type === 'note' ? b.content.text : ''
  const m = /<summary>\n?([\s\S]*?)\n?<\/summary>/.exec(text)
  return m?.[1] ?? text
})

const fresh = !!props.node && Date.now() - props.node.at < FRESH_MS
/** The size after, as shown: counting down from the size before when fresh. */
const after = ref(fresh ? (data.value?.before ?? 0) : (data.value?.after ?? 0))
const now = ref(Date.now())
let frame = 0
let timer: ReturnType<typeof setInterval> | undefined

onMounted(() => {
  if (props.live) timer = setInterval(() => (now.value = Date.now()), 1000)
  const d = data.value
  if (!fresh || !d || matchMedia('(prefers-reduced-motion: reduce)').matches) {
    after.value = d?.after ?? 0
    return
  }
  const start = performance.now()
  const step = (at: number): void => {
    const k = Math.min(1, (at - start) / COUNT_MS)
    const ease = 1 - (1 - k) ** 3
    after.value = Math.round(d.before + (d.after - d.before) * ease)
    if (k < 1) frame = requestAnimationFrame(step)
  }
  frame = requestAnimationFrame(step)
})
onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  clearInterval(timer)
})

const liveLabel = computed(() => {
  const l = props.live!
  const took = fmtThought(Math.max(0, now.value - l.since))
  return l.before
    ? t('chat.compaction.liveSized', { before: tokens.format(l.before), took })
    : t('chat.compaction.live', { took })
})
</script>

<template>
  <div v-if="live" class="cline is-live" role="status">
    <span class="cline-rule" />
    <span class="flex flex-none items-center gap-2 tabular-nums">
      <span class="dot-live size-1.5 rounded-full bg-brand" />{{ liveLabel }}
    </span>
    <span class="cline-rule is-end" />
  </div>
  <div v-else class="flex min-w-0 flex-col gap-2">
    <button
      type="button"
      class="cline transition-colors hover:text-ink"
      :class="{ 'is-fresh': fresh }"
      :aria-expanded="open"
      :title="open ? t('chat.compaction.hide') : t('chat.compaction.show')"
      @click="open = !open"
    >
      <span class="cline-rule" />
      <span class="flex flex-none items-center gap-1.5 tabular-nums">
        <Icon name="history" :size="12" />
        {{
          t('chat.compaction.line', {
            before: tokens.format(data?.before ?? 0),
            after: tokens.format(after),
          })
        }}
        <Icon
          name="down"
          :size="12"
          class="transition-transform duration-150"
          :class="open ? 'rotate-180' : ''"
        />
      </span>
      <span class="cline-rule is-end" />
    </button>
    <div
      v-if="open"
      v-enter="'enter-fade'"
      class="md rounded-control border border-line-soft bg-surface-2 px-4 py-3 text-meta text-ink-2"
      v-html="renderMarkdown(summary)"
    />
  </div>
</template>

<style scoped>
/* one line, live or written, so the live one gives way without a jump */
.cline {
  display: flex;
  width: 100%;
  min-width: 0;
  height: 1.5rem;
  align-items: center;
  gap: 0.75rem;
  font-size: var(--text-meta);
  color: var(--ink-3);
}

/* a hairline tapering off towards the column's edge; the end one is the
 * start one mirrored, so whatever runs along a rule runs inwards on both.
 * The box is taller than the line it draws, room for a pulse's glow. */
.cline-rule {
  position: relative;
  height: 7px;
  flex: 1;
  overflow: hidden;
  background: linear-gradient(var(--line), var(--line)) center / 100% 1px no-repeat;
  mask-image: linear-gradient(to right, transparent, #000 30%);
}
.cline-rule.is-end {
  scale: -1 1;
}
.cline-rule::after {
  content: '';
  position: absolute;
  inset-block: 3px;
  left: 0;
  width: 100%;
  opacity: 0;
}

/* live: coral pulses gathered into the middle. The pulse is the rule's own
 * length with its head at the end, so it runs from the outer edge to the
 * inner one at any width and never leaves the line. */
.is-live .cline-rule::after {
  background: linear-gradient(to right, transparent 55%, var(--brand));
  filter: drop-shadow(0 0 2px color-mix(in srgb, var(--brand) 60%, transparent));
  animation: inflow 1.6s linear infinite;
}

/* just written: one pulse let go from the middle, back out */
.is-fresh .cline-rule::after {
  left: auto;
  right: 0;
  width: 100%;
  background: linear-gradient(to left, var(--brand), transparent 70%);
  box-shadow: 0 0 3px 0.5px color-mix(in srgb, var(--brand) 45%, transparent);
  transform-origin: right;
  animation: release 1.1s var(--ease-soft) both;
}

@keyframes inflow {
  0% {
    opacity: 0;
    transform: translateX(-100%);
  }
  25%,
  80% {
    opacity: 1;
  }
  100% {
    opacity: 0;
    transform: none;
  }
}

@keyframes release {
  0% {
    opacity: 1;
    transform: scaleX(0);
  }
  100% {
    opacity: 0;
    transform: scaleX(1);
  }
}
</style>
