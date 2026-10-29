<script setup lang="ts">
// Hook trace: which hook fired on which event, and what it did.
// Inject actions that carry a card key get a memory-toned label.
import { t } from '../i18n'
import type { Content, Decision } from '../lib/types'
import { hookTraceForSession } from '../lib/store'
import { relTime, shortId } from '../lib/util'
import Icon from './ui/Icon.vue'

function effectLabel(e: Content): string {
  return e.data?.tag ? `note:${e.data.tag}` : 'note'
}

function decisionLabel(d: Decision): string | null {
  if (d.type === 'block') return `block: ${d.reason.slice(0, 60)}`
  if (d.type === 'rewrite') return 'rewrite'
  return null
}

// Literal utility strings (Tailwind only emits complete class names).
function effectTone(e: Content): string {
  const tag = e.data?.tag
  if (tag?.startsWith('memory') || tag === 'card_use') return 'border-memory/35 bg-memory-soft text-memory'
  return 'border-line-soft bg-canvas text-ink-2'
}

function decisionTone(d: Decision): string {
  return d.type === 'block'
    ? 'border-danger/35 bg-danger-soft text-danger'
    : 'border-line-soft bg-canvas text-ink-2'
}
</script>

<template>
  <div v-if="hookTraceForSession.length" class="flex flex-col gap-1.5">
    <div class="section-title mb-0.5">{{ t('panels.hooks.title') }}</div>
    <div
      v-for="e in [...hookTraceForSession].reverse()"
      :key="e.seq"
      class="rounded-card bg-surface px-3 py-2 text-meta shadow-soft"
    >
      <details class="group/trace">
        <summary
          class="flex cursor-pointer list-none items-center gap-1.5 select-none [&::-webkit-details-marker]:hidden"
        >
          <Icon
            name="chevron"
            :size="10"
            class="text-ink-3 transition-transform group-open/trace:rotate-90"
          />
          <span class="font-mono text-tool">{{ e.event }}</span>
          <span class="text-ink-2">{{ e.plugin }}</span>
          <span class="ml-auto flex-none text-micro text-ink-3"
            >{{ relTime(e.at) }} · {{ shortId(e.branch) }}</span
          >
        </summary>
        <div class="reveal mt-1.5 font-mono text-micro text-ink-3">{{ e.hook_id }}</div>
        <div v-if="e.effects.length || decisionLabel(e.decision)" class="reveal mt-1.5 flex flex-wrap gap-1">
          <span
            v-for="(a, i) in e.effects"
            :key="i"
            class="max-w-full truncate rounded-xs border px-1.5 font-mono text-micro"
            :class="effectTone(a)"
          >
            {{ effectLabel(a) }}
          </span>
          <span
            v-if="decisionLabel(e.decision)"
            class="max-w-full truncate rounded-xs border px-1.5 font-mono text-micro"
            :class="decisionTone(e.decision)"
          >
            {{ decisionLabel(e.decision) }}
          </span>
        </div>
      </details>
    </div>
  </div>
</template>
