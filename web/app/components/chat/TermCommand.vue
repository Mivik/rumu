<script setup lang="ts">
// A command as a terminal shows it: `$`, bash highlighting, a copy button.
// A command past three lines folds, and the `$` becomes its toggle.
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { t } from '../../i18n'
import { highlightLine } from '../../lib/markdown'
import { toast } from '../../lib/state/ui'
import { copyText } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{ command: string }>()

const emit = defineEmits<{ toggle: [expanded: boolean] }>()

const pre = ref<HTMLPreElement | null>(null)
const expanded = ref(false)
const overflowing = ref(false)

const LINES = 3

const folded = computed(() => !expanded.value)
const toggleLabel = computed(() => (expanded.value ? t('common.foldCommand') : t('common.unfoldCommand')))

function measure(): void {
  const el = pre.value
  if (el && folded.value) overflowing.value = el.scrollHeight - el.clientHeight > 2
}

let observer: ResizeObserver | undefined
onMounted(() => {
  void nextTick(measure)
  observer = new ResizeObserver(measure)
  if (pre.value) observer.observe(pre.value)
})
onBeforeUnmount(() => observer?.disconnect())

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
        <span class="glyph chev"><Icon :name="expanded ? 'down' : 'chevron'" :size="12" /></span>
        <span class="glyph dollar">$</span>
      </button>
      <span v-else class="text-ink-3 select-none">$</span>

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
      :class="{ folded, faded: folded && overflowing }"
      :style="folded ? { maxHeight: `calc(1.55em * ${LINES})` } : undefined"
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
  margin: 0;
  font: inherit;
  line-height: 1.55;
  color: var(--ink);
  white-space: pre-wrap;
  word-break: break-word;
}

pre.folded {
  overflow: hidden;
}

pre.faded {
  mask-image: linear-gradient(to bottom, #000 62%, transparent);
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

.mark > .chev,
.group:hover .mark > .dollar,
.mark:focus-visible > .dollar {
  opacity: 0;
}

.group:hover .mark > .chev,
.mark:focus-visible > .chev,
.mark.is-on > .chev {
  opacity: 1;
}

.group:hover .mark > .chev {
  color: var(--ink-2);
}

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
