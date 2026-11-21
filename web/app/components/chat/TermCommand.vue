<script setup lang="ts">
// One command line as a terminal shows it: a `$` prompt, the command with
// bash highlighting, a way to unfold it when it runs long, and a copy button.
// An expanded shell row and a background task's panel both use it, so a
// command reads the same wherever it shows — the task's own label is a
// summary (`description` first, cut to 80 chars), the command is only here.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import { highlightLine } from '../../lib/markdown'
import { toast } from '../../lib/state/ui'
import { copyText } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{
  /** The command as it was called (may span lines). */
  command: string
  /** Fold past this many lines, with a toggle (unset: the whole command,
   * scrolling inside a fixed ceiling). */
  lines?: number
}>()

/** The toggle was pressed: the box around the command may want to react (the
 * task panel scrolls back to the command it just unfolded). */
const emit = defineEmits<{ toggle: [expanded: boolean] }>()

const pre = ref<HTMLPreElement | null>(null)
const expanded = ref(false)
/** The command does not fit folded: measured on the folded block, so the
 * toggle shows only when there is something to unfold. */
const overflowing = ref(false)

const folded = computed(() => props.lines !== undefined && !expanded.value)
/** Folded: exactly `lines` lines, the rest behind the toggle. */
const foldStyle = computed(() =>
  props.lines !== undefined && !expanded.value ? { maxHeight: `calc(1.55em * ${props.lines})` } : undefined,
)
const toggleLabel = computed(() => (expanded.value ? t('common.foldCommand') : t('common.unfoldCommand')))

function measure(): void {
  const el = pre.value
  // Only meaningful while folded: an unfolded block is already showing
  // everything, so whatever was measured when it was folded still stands.
  if (!el || !folded.value) return
  overflowing.value = el.scrollHeight - el.clientHeight > 2
}

let observer: ResizeObserver | undefined
onMounted(() => {
  void nextTick(measure)
  // A width change (the panel resized, the window narrowed) reflows the
  // command onto other lines, so the fit is measured again.
  observer = new ResizeObserver(measure)
  if (pre.value) observer.observe(pre.value)
})
onBeforeUnmount(() => observer?.disconnect())

// Another task's command: never arrive already unfolded.
watch(
  () => props.command,
  () => {
    expanded.value = false
    void nextTick(measure)
  },
)

function toggle(): void {
  expanded.value = !expanded.value
  emit('toggle', expanded.value)
}

async function copy(): Promise<void> {
  const ok = await copyText(props.command)
  toast('info', ok ? t('common.copiedCode') : t('common.copyFailed'))
}
</script>

<template>
  <div class="group flex items-start gap-1.5">
    <!-- The prompt is the control: hovered (or focused), `$` becomes the
         chevron that opens or folds the block. What a terminal would call
         "there is more" is the faded last line of a folded command. -->
    <div class="flex w-3.5 flex-none flex-col items-center self-stretch">
      <button
        v-if="overflowing"
        type="button"
        class="mark"
        :title="toggleLabel"
        :aria-label="toggleLabel"
        :aria-expanded="expanded"
        @click="toggle"
      >
        <span class="glyph chev">
          <Icon :name="expanded ? 'down' : 'chevron'" :size="12" />
        </span>
        <span class="glyph dollar">$</span>
      </button>
      <span v-else class="select-none text-ink-3">$</span>

      <!-- Open: the command runs on down a rule, and the rule ends with the
           way back — the top and the bottom of a long block both fold it. -->
      <template v-if="expanded">
        <span class="my-1 w-px flex-1 bg-line" aria-hidden="true"></span>
        <button
          type="button"
          class="mark is-on"
          :title="t('common.foldCommand')"
          :aria-label="t('common.foldCommand')"
          @click="toggle"
        >
          <span class="glyph chev"><Icon name="chev-up" :size="12" /></span>
        </button>
      </template>
    </div>

    <pre
      ref="pre"
      class="shell min-w-0 flex-1"
      :class="{ folded, open: expanded }"
      :style="foldStyle"
      v-html="highlightLine(command, 'bash')"
    ></pre>

    <button
      type="button"
      class="copy"
      :title="t('common.copyCode')"
      :aria-label="t('common.copyCode')"
      @click="copy"
    >
      <Icon name="copy" :size="12" />
    </button>
  </div>
</template>

<style scoped>
pre {
  max-height: 160px;
  overflow: auto;
  margin: 0;
  font: inherit;
  line-height: 1.55;
  color: var(--ink);
  white-space: pre-wrap;
  word-break: break-word;
}

/* Folded: the ceiling is inline (it follows `lines`), nothing scrolls, and
   the last line fades out — the only cue, at three lines, that the command
   goes on. */
pre.folded {
  overflow: hidden;
  mask-image: linear-gradient(to bottom, #000 62%, transparent);
}

/* Unfolded: as tall as it needs. The box it sits in owns the scrolling — a
   panel puts the command and the output in one box, so a ceiling here would
   only create a second scrollbar. */
pre.open {
  max-height: none;
  overflow: visible;
}

.mark {
  display: grid;
  place-items: center;
  width: 100%;
  padding: 0;
  line-height: inherit;
  color: var(--ink-3);
  border-radius: 4px;
  cursor: pointer;
}

.mark > .glyph {
  grid-area: 1 / 1;
  display: grid;
  place-items: center;
  transition: opacity 0.1s;
}

.mark > .chev {
  opacity: 0;
}

/* A pointer anywhere over the command turns the prompt into the way to fold
   it; the prompt alone is a small target, and reading a command means
   hovering it. */
.group:hover .mark > .dollar,
.mark:focus-visible > .dollar {
  opacity: 0;
}

.group:hover .mark > .chev,
.mark:focus-visible > .chev {
  opacity: 1;
}

/* The bottom chevron has no prompt to swap with: it is the control whenever
   the block is open. */
.mark.is-on > .chev {
  opacity: 1;
}

.group:hover .mark > .chev {
  color: var(--ink-2);
}

/* No hover to reveal the chevron with. */
@media (hover: none) {
  .mark > .chev {
    opacity: 1;
  }

  .mark > .dollar {
    opacity: 0;
  }
}

.copy {
  flex: none;
  padding: 3px;
  border-radius: 4px;
  color: var(--ink-3);
  opacity: 0;
  transition:
    opacity 0.1s,
    background-color 0.1s;
}

.group:hover .copy,
.copy:focus-visible {
  opacity: 1;
}

.copy:hover {
  background: var(--surface);
  color: var(--ink-2);
}
</style>
