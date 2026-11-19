<script setup lang="ts">
// The stats view: the open branch's usage, narrowed to a range of replies or
// across the whole path. Totals and where the time went on top, every model
// call of the range as a column, then the replies as rows — a click narrows
// to one, a shift-click stretches the range to it. The range lives in the
// address (`?view=stats&from=&to=`), so a reply's usage card links here and
// Back returns. The whole session (every branch) closes it as one line.
import { computed } from 'vue'
import { t } from '../../i18n'
import { modelByRef } from '../../lib/state/models'
import { pathNodes, repliesOf, showStats, state, summarize } from '../../lib/store'
import type { ReplyStat } from '../../lib/store'
import { contentText, fmtBytes, fmtDuration, fmtTokens, preview } from '../../lib/util'
import CallBars from '../stats/CallBars.vue'
import TimeSplit from '../stats/TimeSplit.vue'
import TokenChips from '../stats/TokenChips.vue'

const replies = computed(() => repliesOf(pathNodes.value))

/** The range as indices into `replies`; the whole path when unset or stale. */
const span = computed<[number, number] | null>(() => {
  const r = state.statsRange
  if (!r) return null
  const a = replies.value.findIndex((x) => x.id === r.from)
  const b = replies.value.findIndex((x) => x.id === r.to)
  if (a < 0 || b < 0) return null
  return a <= b ? [a, b] : [b, a]
})

const picked = computed(() => {
  const s = span.value
  return s ? replies.value.slice(s[0], s[1] + 1) : replies.value
})
const sum = computed(() => summarize(picked.value))
const calls = computed(() => picked.value.flatMap((r) => r.calls))

const rangeLabel = computed(() => {
  const s = span.value
  if (!s) return t('usage.allReplies', { n: replies.value.length })
  const [a, b] = s
  return a === b ? t('usage.reply', { n: a + 1 }) : t('usage.replyRange', { from: a + 1, to: b + 1 })
})

function inRange(i: number): boolean {
  const s = span.value
  return !!s && i >= s[0] && i <= s[1]
}

function pick(i: number, e: MouseEvent): void {
  const r = replies.value[i]!
  const s = span.value
  if (e.shiftKey && s) {
    // Stretch from the end of the range farther from the click.
    const anchor = i < s[0] ? s[1] : s[0]
    const [a, b] = anchor <= i ? [anchor, i] : [i, anchor]
    showStats({ from: replies.value[a]!.id, to: replies.value[b]!.id })
  } else if (s && s[0] === i && s[1] === i) {
    showStats(null)
  } else {
    showStats({ from: r.id, to: r.id })
  }
}

const pct = (n: number | null): string => (n === null ? '—' : `${Math.round(n * 100)}%`)
const ms = (n: number | null): string => (n === null ? '—' : fmtDuration(n))

const tiles = computed(() => {
  const s = sum.value
  const ctx = modelByRef(s.models.at(-1)?.ref ?? '')?.context
  return [
    { label: t('usage.time'), value: fmtDuration(s.activeMs) },
    { label: t('usage.modelCalls'), value: String(s.calls) },
    {
      label: t('usage.rate'),
      value: s.rate === null ? '—' : String(Math.round(s.rate)),
      unit: s.rate === null ? undefined : t('usage.unitRate'),
    },
    {
      label: t('usage.input'),
      value: fmtTokens(s.input),
      unit: t('usage.unitTokens'),
      sub: `${t('usage.context')} ${fmtTokens(s.lastInput)}${ctx ? ` / ${fmtTokens(ctx)}` : ''}`,
    },
    { label: t('usage.output'), value: fmtTokens(s.output), unit: t('usage.unitTokens') },
    {
      label: t('usage.hitRate'),
      value: pct(s.hitRate),
      sub: s.written ? `${t('usage.written')} ${fmtTokens(s.written)}` : undefined,
    },
    {
      label: t('usage.requestSize'),
      value: s.lastBytes === null ? '—' : fmtBytes(s.lastBytes),
      sub: s.totalBytes ? t('usage.totalBytes', { size: fmtBytes(s.totalBytes) }) : undefined,
    },
    {
      label: t('usage.ttftAvg'),
      value: ms(s.avgTtftMs),
      sub: s.maxTtftMs === null ? undefined : t('usage.ttftMax', { ms: fmtDuration(s.maxTtftMs) }),
    },
    {
      label: t('usage.retries'),
      value: String(s.retries),
      sub: [
        s.failures ? t('usage.failures', { n: s.failures }) : '',
        s.resets ? t('usage.resets', { n: s.resets, count: s.resets }) : '',
      ]
        .filter(Boolean)
        .join(' · '),
      warn: s.retries > 0,
      subWarn: s.failures > 0 || s.resets > 0,
    },
  ]
})

const models = computed(() =>
  sum.value.models.map((m) => ({
    ...m,
    name: modelByRef(m.ref)?.name ?? m.ref.slice(m.ref.indexOf('/') + 1),
  })),
)

function promptText(r: ReplyStat): string {
  return r.prompt ? preview(contentText(r.prompt.body.content), 80) : ''
}

function replyRow(r: ReplyStat) {
  const s = summarize([r])
  return {
    calls: s.calls,
    time: s.activeMs,
    ttft: s.avgTtftMs,
    output: s.output,
    hit: s.hitRate,
  }
}
const rows = computed(() => replies.value.map((r) => ({ r, ...replyRow(r) })))

/** Every branch of the session, sub-agents included: totalled by the server,
 *  since the tree leaves sub-agents' nodes out. */
const session = computed(() => state.usage)
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div class="scroll-fade flex-1 overflow-y-auto pt-3 pb-6">
      <div
        :key="state.sessionId ?? ''"
        class="view-in mx-auto flex max-w-[var(--col-max)] flex-col px-4 sm:px-6"
      >
        <div v-if="!replies.length" class="py-10 text-center text-meta text-ink-3">
          {{ t('usage.empty') }}
        </div>
        <template v-else>
          <div class="flex items-center gap-2">
            <span class="text-meta font-semibold text-ink">{{ rangeLabel }}</span>
            <button
              v-if="span"
              type="button"
              class="rounded-full bg-surface-2 px-2 py-0.5 text-micro font-semibold text-ink-2 transition-colors hover:text-ink"
              @click="showStats(null)"
            >
              {{ t('usage.showAll') }}
            </button>
          </div>

          <div class="mt-3 grid grid-cols-2 gap-1.5 sm:grid-cols-3">
            <div v-for="f in tiles" :key="f.label" class="min-w-0 rounded-control bg-surface-2 px-3 py-2.5">
              <div class="truncate text-micro text-ink-3">{{ f.label }}</div>
              <div class="mt-0.5 flex items-baseline gap-1 whitespace-nowrap">
                <span
                  class="text-title leading-none font-black tabular-nums"
                  :class="f.warn ? 'text-warn' : 'text-ink'"
                  >{{ f.value }}</span
                >
                <span v-if="f.unit" class="text-micro font-semibold text-ink-3">{{ f.unit }}</span>
              </div>
              <div
                v-if="f.sub"
                class="mt-1 truncate text-micro tabular-nums"
                :class="f.subWarn ? 'text-warn' : 'text-ink-3'"
              >
                {{ f.sub }}
              </div>
            </div>
          </div>

          <TimeSplit :summary="sum" class="mt-4" />

          <CallBars :calls="calls" size="lg" :reply-labels="picked.length > 1" class="mt-4">
            <template #idle>
              <TokenChips
                :cached="sum.cached"
                :written="sum.written"
                :uncached="Math.max(0, sum.input - sum.cached - sum.written)"
                :output="sum.output"
              />
            </template>
          </CallBars>

          <div v-if="models.length" class="mt-3">
            <div class="section-title mb-1">{{ t('usage.models') }}</div>
            <div v-for="m in models" :key="m.ref" class="flex items-baseline gap-2 px-1 text-meta">
              <span class="font-semibold text-ink">{{ m.name }}</span>
              <span class="min-w-0 truncate font-mono text-micro text-ink-3">{{ m.ref }}</span>
              <span class="ml-auto flex-none tabular-nums text-ink-2">{{
                t('usage.calls', { n: m.calls, count: m.calls })
              }}</span>
            </div>
          </div>

          <div class="section-title mt-5 mb-1">{{ t('usage.replies') }}</div>
          <table class="w-full table-fixed text-meta tabular-nums">
            <thead>
              <tr class="text-left text-micro text-ink-3">
                <th class="w-8 py-1 pl-1 font-normal">#</th>
                <th class="py-1 font-normal" />
                <th class="w-14 py-1 text-right font-normal">{{ t('usage.modelCalls') }}</th>
                <th class="w-16 py-1 text-right font-normal">{{ t('usage.time') }}</th>
                <th class="hidden w-16 py-1 text-right font-normal sm:table-cell">{{ t('usage.ttft') }}</th>
                <th class="w-14 py-1 text-right font-normal">{{ t('usage.output') }}</th>
                <th class="w-14 py-1 pr-1 text-right font-normal">{{ t('usage.hitRate') }}</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="(row, i) in rows"
                :key="row.r.id"
                class="cursor-pointer border-t border-line-soft transition-colors select-none"
                :class="inRange(i) ? 'bg-surface-2' : 'hover:bg-surface-2/60'"
                :aria-selected="inRange(i)"
                @click="pick(i, $event)"
              >
                <td class="py-1.5 pl-1 text-micro text-ink-3">{{ row.r.index }}</td>
                <td class="truncate py-1.5 pr-2" :class="row.r.prompt ? 'text-ink' : 'text-ink-3 italic'">
                  {{ promptText(row.r) || t('usage.continued') }}
                </td>
                <td class="py-1.5 text-right text-ink-2">{{ row.calls }}</td>
                <td class="py-1.5 text-right text-ink-2">{{ fmtDuration(row.time) }}</td>
                <td class="hidden py-1.5 text-right text-ink-2 sm:table-cell">{{ ms(row.ttft) }}</td>
                <td class="py-1.5 text-right text-ink-2">{{ fmtTokens(row.output) }}</td>
                <td class="py-1.5 pr-1 text-right text-ink-2">{{ pct(row.hit) }}</td>
              </tr>
            </tbody>
          </table>

          <div class="mt-5 border-t border-line-soft pt-2 text-micro tabular-nums text-ink-3">
            <span class="font-semibold text-ink-2">{{ t('usage.session') }}</span>
            ·
            {{
              t('usage.sessionLine', {
                calls: session.calls,
                input: fmtTokens(session.input),
                cached: fmtTokens(session.cached),
                output: fmtTokens(session.output),
              })
            }}
          </div>
        </template>
      </div>
    </div>
  </div>
</template>
