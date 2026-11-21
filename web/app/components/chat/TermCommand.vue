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

function measure(): void {
  const el = pre.value
  // Only meaningful while folded: an unfolded block is already showing
  // everything up to its ceiling, which is what the toggle is for.
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
    <span class="flex-none text-ink-3 select-none">$</span>
    <pre
      ref="pre"
      class="shell min-w-0 flex-1"
      :class="{ folded, open: expanded }"
      :style="foldStyle"
      v-html="highlightLine(command, 'bash')"
    ></pre>
    <span class="flex flex-none items-start">
      <button
        v-if="overflowing"
        type="button"
        class="term-btn"
        :title="expanded ? t('common.foldCommand') : t('common.unfoldCommand')"
        :aria-label="expanded ? t('common.foldCommand') : t('common.unfoldCommand')"
        :aria-expanded="expanded"
        @click="toggle"
      >
        <Icon :name="expanded ? 'fold-v' : 'unfold-v'" :size="12" />
      </button>
      <button
        type="button"
        class="term-btn term-copy"
        :title="t('common.copyCode')"
        :aria-label="t('common.copyCode')"
        @click="copy"
      >
        <Icon name="copy" :size="12" />
      </button>
    </span>
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

/* Folded: the ceiling is inline (it follows `lines`), and nothing scrolls —
   the toggle is the way to see the rest. */
pre.folded {
  overflow: hidden;
}

/* Unfolded: as tall as it needs. The box it sits in owns the scrolling — a
   panel puts the command and the output in one box, so a ceiling here would
   only create a second scrollbar. */
pre.open {
  max-height: none;
  overflow: visible;
}

.term-btn {
  flex: none;
  padding: 3px;
  border-radius: 4px;
  color: var(--ink-3);
  transition:
    opacity 0.1s,
    background-color 0.1s,
    color 0.1s;
}

.term-btn:hover {
  background: var(--surface);
  color: var(--ink-2);
}

/* The unfolding toggle stays visible: on a touch screen there is no hover to
   reveal it with. The copy button is quiet until asked for. */
.term-copy {
  opacity: 0;
}

.group:hover .term-copy,
.term-copy:focus-visible {
  opacity: 1;
}
</style>
