<script setup lang="ts">
// The body of an expanded `read` row: the text it returned, numbered from
// the line the call started at (`offset`, or a `#L<n>` anchor on the path)
// and highlighted as one block in the file's language, so multi-line
// strings and comments color right. The output cap's "[truncated …]"
// marker is lifted out of the code into a note. Same paper as FileDiff.
import { computed } from 'vue'
import { t } from '../../i18n'
import { highlightLine, langForPath } from '../../lib/markdown'
import { splitLines } from '../../lib/diff'

const props = defineProps<{
  text: string
  /** The called path (may carry a `<ws>:` prefix and a `#L` anchor) — picks the grammar. */
  path: string
  args: Record<string, unknown> | null
}>()

const ANCHOR = /#L(\d+)(?:-L\d+)?$/
const TRUNCATED = /\n… \[truncated, \d+ of \d+ chars\]$/

const parts = computed(() => {
  const m = TRUNCATED.exec(props.text)
  return m ? { body: props.text.slice(0, m.index), note: m[0].trim() } : { body: props.text, note: '' }
})

const lines = computed(() => splitLines(parts.value.body))

const start = computed(() => {
  const offset = props.args?.offset
  if (typeof offset === 'number' && offset >= 1) return offset
  const path = props.args?.path
  return Number((typeof path === 'string' && ANCHOR.exec(path)?.[1]) || 1)
})

const numbers = computed(() => lines.value.map((_, i) => start.value + i).join('\n'))

const html = computed(() =>
  highlightLine(lines.value.join('\n'), langForPath(props.path.replace(ANCHOR, ''))),
)
</script>

<template>
  <div class="fview">
    <div v-if="lines.length" class="fview-body">
      <div class="fview-num">{{ numbers }}</div>
      <pre class="fview-code" v-html="html"></pre>
    </div>
    <div v-else class="fview-note">{{ t('common.empty') }}</div>
    <div v-if="parts.note" class="fview-note">{{ parts.note }}</div>
  </div>
</template>

<style scoped>
/* Edge to edge under the row line, like FileDiff: the gutter stays put
 * while long lines scroll sideways. */
.fview {
  border-top: 1px solid var(--line-soft);
  background: var(--surface);
  font-family: var(--font-mono);
  font-size: var(--text-micro);
  line-height: 1.55;
}

.fview-body {
  display: flex;
  max-height: 360px;
  overflow: auto;
}

.fview-num {
  position: sticky;
  left: 0;
  flex: none;
  min-width: calc(3ch + 6px);
  padding-right: 1.5ch;
  background: var(--surface);
  color: var(--ink-3);
  font-variant-numeric: tabular-nums;
  text-align: right;
  white-space: pre;
  user-select: none;
}

.fview-code {
  flex: none;
  margin: 0;
  padding-right: 12px;
  font: inherit;
  white-space: pre;
}

.fview-note {
  padding: 3px 10px;
  color: var(--ink-3);
}
.fview-body + .fview-note {
  border-top: 1px solid var(--line-soft);
}
</style>
