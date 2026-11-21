<script setup lang="ts">
// One command line as a terminal shows it: a `$` prompt, the command with
// bash highlighting, and a copy button on hover. An expanded shell row and a
// background task's panel both use it, so a command reads the same wherever
// it shows — the task's own label is a summary (`description` first, cut to
// 80 chars), the command is only ever here.
import { t } from '../../i18n'
import { highlightLine } from '../../lib/markdown'
import { toast } from '../../lib/state/ui'
import { copyText } from '../../lib/util'
import Icon from '../ui/Icon.vue'

const props = defineProps<{
  /** The command as it was called (may span lines). */
  command: string
}>()

async function copy(): Promise<void> {
  const ok = await copyText(props.command)
  toast('info', ok ? t('common.copiedCode') : t('common.copyFailed'))
}
</script>

<template>
  <div class="group flex items-start gap-1.5">
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
</template>

<style scoped>
pre {
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
</style>
