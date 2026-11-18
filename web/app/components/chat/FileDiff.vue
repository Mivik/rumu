<script setup lang="ts">
// The Claude-Code-style diff of an expanded write/edit/delete row: two line-number
// gutters, a sign column, the code highlighted per line in the file's
// language. Renders the result's FilePatch hunks; without them it falls
// back to the call's own strings — a legacy edit diffed in the browser (no
// line numbers: they would be made up), a create's content as all-added.
// Rows cap at MAX_ROWS with a "N more lines" note so huge files stay fast.
// `clip` (an edit standing in the conversation, EditCard) shows a small diff
// whole and a big one's first CLIP_ROWS under a fade, with a footer that
// says how much is left and where its other hunks start; either opens it
// all, the hunk at that hunk. A clipped diff scrolls with the page, no box.
import { computed, nextTick, ref } from 'vue'
import { t } from '../../i18n'
import type { FilePatch } from '../../lib/types'
import { diffLines, splitLines } from '../../lib/diff'
import { highlightLine, langForPath } from '../../lib/markdown'

const props = withDefaults(
  defineProps<{
    patch: FilePatch | null
    /** The called path (may carry a `<ws>:` prefix) — picks the grammar. */
    path: string
    args: Record<string, unknown> | null
    clip?: boolean
  }>(),
  { clip: false },
)

const MAX_ROWS = 500
/** A longer line shows this much and how much is left: one minified line
 *  (a JSON dump, a bundle) runs to hundreds of KB — tens of thousands of
 *  highlighted spans on one unwrapped line, seconds of layout. */
const MAX_LINE = 1000
/** A clipped diff this long or shorter (context and gaps included) shows whole. */
const FULL_ROWS = 40
/** What a longer one shows before its fade. */
const CLIP_ROWS = 24

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
  // An update or delete whose hunks were dropped (too big to diff, not
  // text) shows a note; a legacy result falls back to its args — an edit's
  // strings, a content.
  if (props.patch?.kind === 'update' || props.patch?.kind === 'delete')
    return [{ kind: 'note', text: t('tools.diff.unavailable') }]
  if (strArg('old_string') || strArg('new_string')) {
    return diffLines(strArg('old_string'), strArg('new_string')).map((l) => line(l.sign, l.text, null, null))
  }
  return splitLines(strArg('content')).map((text, i) => line('+', text, null, i + 1))
})

/** A clipped diff the user opened in full. */
const unclipped = ref(false)
const clipped = computed(() => props.clip && !unclipped.value && rows.value.length > FULL_ROWS)
const limit = computed(() => (clipped.value ? CLIP_ROWS : MAX_ROWS))

/** Only shown rows are highlighted; reading grammarVersion inside (via
 * highlightLine) re-highlights them once a lazily loaded grammar lands. */
const shown = computed(() => {
  const lang = langForPath(props.path)
  return rows.value.slice(0, limit.value).map((r) => {
    if (r.kind !== 'line') return r
    if (r.text.length <= MAX_LINE) return { ...r, html: highlightLine(r.text, lang), cut: 0 }
    // don't split a surrogate pair
    const end = /[\uD800-\uDBFF]/.test(r.text[MAX_LINE - 1]!) ? MAX_LINE - 1 : MAX_LINE
    return { ...r, html: highlightLine(r.text.slice(0, end), lang), cut: r.text.length - end }
  })
})
const hidden = computed(() => rows.value.length - shown.value.length)

/** The hunks the clip hides, by the row a hunk starts at and its first new
 * line number: the footer's "L88 · L131" jumps. */
const hiddenHunks = computed(() => {
  if (!clipped.value) return []
  const out: { row: number; line: number }[] = []
  rows.value.forEach((r, i) => {
    const next = rows.value[i + 1]
    if (r.kind === 'gap' && i >= CLIP_ROWS && next?.kind === 'line') {
      out.push({ row: i + 1, line: next.new ?? next.old ?? 0 })
    }
  })
  return out
})

const root = ref<HTMLElement | null>(null)
/** Open the whole diff; with a row, bring it into view. */
async function unclip(row?: number): Promise<void> {
  unclipped.value = true
  if (row === undefined) return
  await nextTick()
  root.value?.querySelector(`[data-row="${row}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' })
}

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
    ref="root"
    class="fdiff"
    :class="{ 'is-clip': clip, 'is-clipped': clipped }"
    :style="{ '--fdiff-num': numWidth, '--fdiff-gutters': Number(gutters.old) + Number(gutters.new) }"
  >
    <div v-for="(row, i) in shown" :key="i" class="fdiff-row" :class="rowClass(row)" :data-row="i">
      <template v-if="row.kind === 'line'">
        <span v-if="gutters.old" class="fdiff-num">{{ row.old ?? '' }}</span>
        <span v-if="gutters.new" class="fdiff-num">{{ row.new ?? '' }}</span>
        <span class="fdiff-sign">{{ row.sign === ' ' ? '' : row.sign }}</span>
        <span class="fdiff-code" v-html="row.html"></span>
        <span v-if="row.cut" class="fdiff-cut">{{ t('tools.diff.moreChars', { count: row.cut }) }}</span>
      </template>
      <span v-else-if="row.kind === 'gap'" class="fdiff-gap">⋯</span>
      <span v-else class="fdiff-note">{{ row.text }}</span>
    </div>
    <div v-if="clipped" class="fdiff-clipbar">
      <button type="button" class="fdiff-unclip" @click="unclip()">
        {{ t('tools.diff.showAll', { count: hidden }) }}
      </button>
      <span v-if="hiddenHunks.length" class="fdiff-hunks">
        {{ t('tools.diff.otherHunks') }}
        <button v-for="h in hiddenHunks" :key="h.row" type="button" class="fdiff-hunk" @click="unclip(h.row)">
          L{{ h.line }}
        </button>
      </span>
    </div>
    <div v-else-if="hidden > 0" class="fdiff-more">
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

/* where an over-long line stops */
.fdiff-cut {
  flex: none;
  padding-right: 12px;
  color: var(--ink-3);
  user-select: none;
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

/* In the conversation (EditCard) the diff grows with the page: no inner
 * scroll box but the sideways one. */
.fdiff.is-clip {
  max-height: none;
  overflow-y: hidden;
}

/* the clip's fade over its last rows, under the footer */
.fdiff.is-clipped {
  position: relative;
}

.fdiff-clipbar {
  position: sticky;
  left: 0;
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 12px;
  width: 100%;
  padding: 4px 10px;
  border-top: 1px solid var(--line-soft);
  background: var(--surface);
  font-family: var(--font-sans);
  box-shadow: 0 -28px 24px -4px var(--surface);
}

.fdiff-unclip {
  font-weight: 500;
  color: var(--tool);
}

.fdiff-unclip:hover,
.fdiff-hunk:hover {
  text-decoration: underline;
}

.fdiff-hunks {
  color: var(--ink-3);
}

.fdiff-hunk {
  margin-left: 6px;
  font-family: var(--font-mono);
  color: var(--ink-2);
}
</style>
