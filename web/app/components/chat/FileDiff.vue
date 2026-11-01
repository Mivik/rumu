<script setup lang="ts">
// The Claude-Code-style diff of an expanded write/edit row: two line-number
// gutters, a sign column, the code highlighted per line in the file's
// language. Renders the result's FilePatch hunks; without them it falls
// back to the call's own strings — a legacy edit diffed in the browser (no
// line numbers: they would be made up), a create's content as all-added.
// Rows cap at MAX_ROWS with a "N more lines" note so huge files stay fast.
import { computed } from 'vue'
import { t } from '../../i18n'
import type { FilePatch } from '../../lib/types'
import { diffLines, splitLines } from '../../lib/diff'
import { highlightLine, langForPath } from '../../lib/markdown'

const props = defineProps<{
  patch: FilePatch | null
  /** The called path (may carry a `<ws>:` prefix) — picks the grammar. */
  path: string
  args: Record<string, unknown> | null
}>()

const MAX_ROWS = 500

type Row =
  | { kind: 'line'; sign: ' ' | '-' | '+'; text: string; old: number | null; new: number | null }
  | { kind: 'note'; text: string }
  | { kind: 'gap' }

const strArg = (key: string): string =>
  typeof props.args?.[key] === 'string' ? (props.args[key] as string) : ''

const rows = computed<Row[]>(() => {
  const line = (sign: ' ' | '-' | '+', text: string, oldNum: number | null, newNum: number | null): Row => ({
    kind: 'line',
    sign,
    text,
    old: oldNum,
    new: newNum,
  })
  if (props.patch?.hunks) {
    const out: Row[] = []
    for (const h of props.patch.hunks) {
      if (out.length) out.push({ kind: 'gap' })
      let o = h.old_start
      let n = h.new_start
      for (const raw of h.lines) {
        const sign = raw[0] as ' ' | '-' | '+' | '\\'
        if (sign === '\\') {
          out.push({ kind: 'note', text: raw })
          continue
        }
        out.push(line(sign, raw.slice(1), sign === '+' ? null : o, sign === '-' ? null : n))
        if (sign !== '+') o++
        if (sign !== '-') n++
      }
    }
    return out
  }
  // An update whose hunks were dropped (too big to diff) shows a note; a
  // legacy result falls back to its args — an edit's strings, a content.
  if (props.patch?.kind === 'update') return [{ kind: 'note', text: t('tools.diff.unavailable') }]
  if (strArg('old_string') || strArg('new_string')) {
    return diffLines(strArg('old_string'), strArg('new_string')).map((l) => line(l.sign, l.text, null, null))
  }
  return splitLines(strArg('content')).map((text, i) => line('+', text, null, i + 1))
})

/** Only shown rows are highlighted; reading grammarVersion inside (via
 * highlightLine) re-highlights them once a lazily loaded grammar lands. */
const shown = computed(() => {
  const lang = langForPath(props.path)
  return rows.value
    .slice(0, MAX_ROWS)
    .map((r) => (r.kind === 'line' ? { ...r, html: highlightLine(r.text, lang) } : r))
})
const hidden = computed(() => rows.value.length - shown.value.length)

/** Gutter width in ch, sized to the largest line number shown. */
const numWidth = computed(() => {
  const max = shown.value.reduce((m, r) => (r.kind === 'line' ? Math.max(m, r.old ?? 0, r.new ?? 0) : m), 0)
  return `${Math.max(3, String(max).length)}ch`
})

/** A gutter shows only when some row numbers it: a create has no old
 * side, a legacy browser diff no numbers at all. */
const gutters = computed(() => {
  const lines = shown.value.filter((r) => r.kind === 'line')
  return { old: lines.some((r) => r.old !== null), new: lines.some((r) => r.new !== null) }
})

function rowClass(row: Row): string {
  return row.kind === 'line' ? (row.sign === '+' ? 'is-add' : row.sign === '-' ? 'is-del' : '') : ''
}
</script>

<template>
  <div
    v-if="rows.length"
    class="fdiff"
    :style="{ '--fdiff-num': numWidth, '--fdiff-gutters': Number(gutters.old) + Number(gutters.new) }"
  >
    <div v-for="(row, i) in shown" :key="i" class="fdiff-row" :class="rowClass(row)">
      <template v-if="row.kind === 'line'">
        <span v-if="gutters.old" class="fdiff-num">{{ row.old ?? '' }}</span>
        <span v-if="gutters.new" class="fdiff-num">{{ row.new ?? '' }}</span>
        <span class="fdiff-sign">{{ row.sign === ' ' ? '' : row.sign }}</span>
        <span class="fdiff-code" v-html="row.html"></span>
      </template>
      <span v-else-if="row.kind === 'gap'" class="fdiff-gap">⋯</span>
      <span v-else class="fdiff-note">{{ row.text }}</span>
    </div>
    <div v-if="hidden > 0" class="fdiff-more">
      {{ t('tools.diff.moreLines', { count: hidden }) }}
    </div>
  </div>
</template>

<style scoped>
/* The body of an expanded write/edit row, edge to edge under the row line:
 * the paper's own surface so the +/- tints (pine / danger, mixed over it)
 * read as rows of the file. Rows are as wide as their longest line and
 * scroll sideways together. */
.fdiff {
  --fdiff-num: 4ch;
  --fdiff-gutters: 2;
  max-height: 360px;
  overflow: auto;
  border-top: 1px solid var(--line-soft);
  background: var(--surface);
  font-family: var(--font-mono);
  font-size: var(--text-micro);
  line-height: 1.55;
}

.fdiff-row {
  display: flex;
  width: max-content;
  min-width: 100%;
  white-space: pre;
}

.fdiff-num {
  flex: none;
  width: calc(var(--fdiff-num) + 6px);
  text-align: right;
  color: var(--ink-3);
  font-variant-numeric: tabular-nums;
  user-select: none;
}

.fdiff-sign {
  flex: none;
  width: 2ch;
  text-align: center;
  user-select: none;
}

.fdiff-code {
  flex: none;
  padding-right: 12px;
}

.fdiff-row.is-add {
  background: color-mix(in srgb, var(--pine) 9%, transparent);
}

.fdiff-row.is-add .fdiff-sign {
  color: var(--pine);
}

.fdiff-row.is-del {
  background: color-mix(in srgb, var(--danger) 9%, transparent);
}

.fdiff-row.is-del .fdiff-sign {
  color: var(--danger);
}

/* the ellipsis between hunks: centered across the full row width */
.fdiff-gap {
  flex: 1;
  color: var(--ink-3);
  text-align: center;
  user-select: none;
}

/* the "\ No newline at end of file" note, indented under its line */
.fdiff-note {
  flex: 1;
  padding-left: calc(var(--fdiff-gutters) * (var(--fdiff-num) + 6px) + 2ch);
  color: var(--ink-3);
  font-style: italic;
}

.fdiff-more {
  padding: 3px 10px;
  border-top: 1px solid var(--line-soft);
  color: var(--ink-3);
}
</style>
