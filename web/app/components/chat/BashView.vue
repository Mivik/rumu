<script setup lang="ts">
// The body of an expanded `bash` row: the whole command (selectable, with a
// copy button) and its other args as a raw line, the output, and the exit code when it
// isn't 0. A background call passes its task's output and `running` while
// it goes (the row above already says it's running, so nothing here does).
// An output that never ran (a tool error, a hook block) reads in
// danger.
import { computed } from 'vue'
import { t } from '../../i18n'
import { highlightLine } from '../../lib/markdown'
import { vFollow } from '../../lib/motion'
import { toast } from '../../lib/state/ui'
import { bashExit } from '../../lib/toolrows'
import { copyText } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{
  output: string
  /** The call did not succeed (a non-zero exit, a tool error, a block). */
  failed: boolean
  running?: boolean
  /** The command, shown above the output. */
  command?: string
  /** The call's other args (`bashExtraArgs`), as a raw line under it. */
  args?: [string, unknown][]
}>()

/** One arg value on one line, JSON-ish: strings quoted, objects inline. */
function inline(v: unknown): string {
  if (v !== null && typeof v === 'object' && !Array.isArray(v)) {
    const parts = Object.entries(v).map(([k, x]) => `${k}: ${inline(x)}`)
    return `{ ${parts.join(', ')} }`
  }
  return JSON.stringify(v) ?? 'undefined'
}

/** Literal utility strings (Tailwind only emits complete class names). */
function tone(v: unknown): string {
  if (typeof v === 'string') return 'text-pine'
  if (typeof v === 'number') return 'text-tool'
  if (v !== null && typeof v === 'object') return 'text-ink-2'
  return 'text-memory'
}

const result = computed(() => {
  const { code, body } = bashExit(props.output)
  return { code, body: body.replace(/\n+$/, '') }
})

async function copy(): Promise<void> {
  const ok = await copyText(props.command ?? '')
  toast('info', ok ? t('common.copiedCode') : t('common.copyFailed'))
}
</script>

<template>
  <div class="term">
    <div v-if="command" class="term-cmd group">
      <div class="flex items-start gap-1.5">
        <span class="flex-none text-ink-3 select-none">$</span>
        <pre class="shell min-w-0 flex-1" v-html="highlightLine(command, 'bash')"></pre>
        <button
          type="button"
          class="term-copy"
          :title="t('common.copyCode')"
          :aria-label="t('common.copyCode')"
          @click="copy"
        >
          <Icon name="copy" :size="12" />
        </button>
      </div>
      <div v-if="args?.length" class="term-args">
        <span v-for="[k, v] in args" :key="k"
          ><span class="text-ink-3">{{ k }}:</span> <span :class="tone(v)">{{ inline(v) }}</span></span
        >
      </div>
    </div>
    <pre
      v-if="result.body"
      v-follow="running"
      class="term-out"
      :class="{ 'text-danger': failed && result.code === null }"
      >{{ result.body }}</pre>
    <div v-else-if="!running" class="term-foot">{{ t('common.noOutput') }}</div>
    <div v-if="!running && result.code" class="term-foot term-exit">
      {{ t('tools.result.exit', { code: result.code }) }}
    </div>
  </div>
</template>

<style scoped>
/* Edge to edge under the row line, like FileDiff. */
.term {
  border-top: 1px solid var(--line-soft);
  background: var(--surface);
  font-family: var(--font-mono);
  font-size: var(--text-micro);
  line-height: 1.55;
}

.term > * + * {
  border-top: 1px solid var(--line-soft);
}

.term-cmd {
  padding: 4px 6px 4px 10px;
  background: var(--surface-2);
  font-size: var(--text-meta);
}

.term-cmd pre {
  max-height: 160px;
  overflow: auto;
  margin: 0;
  font: inherit;
  color: var(--ink);
  white-space: pre-wrap;
  word-break: break-word;
}

.term-copy {
  flex: none;
  padding: 3px;
  border-radius: 4px;
  color: var(--ink-3);
  opacity: 0;
  transition:
    opacity 0.1s,
    background-color 0.1s;
}

.group:hover .term-copy,
.term-copy:focus-visible {
  opacity: 1;
}

.term-copy:hover {
  background: var(--surface);
  color: var(--ink-2);
}

/* The raw args, small and quiet: an internal readout, not a sentence. */
.term-args {
  display: flex;
  flex-wrap: wrap;
  gap: 0 12px;
  padding-left: 14px;
  font-size: var(--text-micro);
  word-break: break-all;
}

.term-out {
  max-height: 320px;
  overflow: auto;
  margin: 0;
  padding: 4px 10px;
  font: inherit;
  white-space: pre-wrap;
  word-break: break-word;
}

.term-foot {
  padding: 3px 10px;
  color: var(--ink-3);
}

.term-exit {
  color: var(--danger);
  font-weight: 600;
}
</style>
