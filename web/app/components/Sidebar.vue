<script setup lang="ts">
// Left column, straight on the canvas: wordmark, new session, sessions
// (grouped Today / Earlier).
//
// Alignment: the column pads 12px; rows and buttons span it, and every
// line of text starts 12px further in, at 24px — except the new-session row,
// whose plus glyph sits at 24px with the label tucked in behind it.
// The wordmark (RumuMark) breathes while a turn runs, goes hollow when the
// connection drops, and doubles as the way home: clicking it returns to the
// landing page. The new-session row is a quiet ghost line rather than a card —
// it also just lands, and the first prompt typed there starts the session.
// Sessions are plain one-line rows; the open one sits on a sand fill.
import { computed } from 'vue'
import { t } from '../i18n'
import { connected, goHome, isBusy, openSession, sortedSessions, state } from '../lib/store'
import type { SessionInfo } from '../lib/types'
import { layout } from '../lib/layout'
import RumuMark from './brand/RumuMark.vue'
import Icon from './ui/Icon.vue'
import LocaleToggle from './LocaleToggle.vue'

/** The wordmark and the new-session row are both the way home; they close the
 * drawer too on narrow screens. */
function home(): void {
  goHome()
  layout.sidebarOpen = false
}

function pickSession(id: string): void {
  openSession(id)
  layout.sidebarOpen = false
}

const presence = computed(() => {
  if (!connected.value) return { live: false, offline: true, title: t('common.offlineReconnecting') }
  if (isBusy.value) return { live: true, offline: false, title: t('common.working') }
  return { live: false, offline: false, title: t('common.connected') }
})

/** Flat list with group headers mixed in, so one TransitionGroup animates both. */
type Row = { kind: 'head'; key: string; label: string } | { kind: 'session'; key: string; s: SessionInfo }

const rows = computed<Row[]>(() => {
  const midnight = new Date().setHours(0, 0, 0, 0)
  const out: Row[] = []
  let group = ''
  for (const s of sortedSessions.value) {
    const g = s.updated_at >= midnight ? t('chat.sidebar.today') : t('chat.sidebar.earlier')
    if (g !== group) {
      out.push({ kind: 'head', key: `head:${g}`, label: g })
      group = g
    }
    out.push({ kind: 'session', key: s.id, s })
  }
  return out
})
</script>

<template>
  <aside class="flex min-h-0 min-w-0 flex-col px-3">
    <div class="flex h-14 items-center gap-2 px-3 pt-1">
      <button
        class="flex items-center rounded-md transition duration-200 ease-soft hover:opacity-70 active:scale-[.97]"
        :title="presence.title"
        :aria-label="t('chat.sidebar.home')"
        @click="home"
      >
        <RumuMark class="text-ink" :height="21" :live="presence.live" :offline="presence.offline" />
      </button>
      <span class="ml-auto flex items-center gap-1">
        <Transition
          enter-active-class="transition duration-200 ease-soft"
          enter-from-class="translate-x-1 opacity-0"
          leave-active-class="transition duration-200"
          leave-to-class="opacity-0"
        >
          <span v-if="!connected" class="text-micro font-semibold text-danger">{{
            t('common.offline')
          }}</span>
        </Transition>
        <button
          class="icon-btn size-8 lg:hidden"
          :title="t('chat.sidebar.close')"
          :aria-label="t('chat.sidebar.close')"
          @click="layout.sidebarOpen = false"
        >
          <Icon name="close" :size="13" />
        </button>
      </span>
    </div>

    <button
      class="group/new mt-2 flex w-full items-center gap-2.5 rounded-control px-3 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/70 hover:text-ink active:scale-[.98]"
      @click="home"
    >
      <Icon
        name="plus"
        :size="14"
        class="text-ink-3 transition-colors duration-200 group-hover/new:text-ink-2"
      />
      {{ t('chat.sidebar.new') }}
    </button>

    <TransitionGroup
      tag="div"
      class="scroll-fade -mx-3 mt-2 flex flex-1 flex-col gap-0.5 overflow-y-auto px-3 pt-2 pb-3"
      move-class="transition-transform duration-300 ease-soft"
      enter-active-class="transition duration-300 ease-soft"
      enter-from-class="-translate-y-1 opacity-0"
    >
      <template v-for="r in rows" :key="r.key">
        <div
          v-if="r.kind === 'head'"
          class="px-3 pt-2 pb-1 font-display text-micro font-semibold tracking-wide text-ink-3"
        >
          {{ r.label }}
        </div>
        <button
          v-else
          class="flex w-full min-w-0 items-center gap-2 rounded-control px-3 py-1.5 text-left text-secondary transition-colors duration-200"
          :class="
            r.s.id === state.sessionId
              ? 'bg-surface-2 font-medium text-ink'
              : 'text-ink-2 hover:bg-surface-2/60 hover:text-ink'
          "
          @click="pickSession(r.s.id)"
        >
          <span class="min-w-0 flex-1 truncate">{{ r.s.title || t('common.untitled') }}</span>
          <span
            v-if="r.s.id === state.sessionId && isBusy"
            class="dot-live size-1.5 flex-none rounded-full bg-brand"
            :title="t('common.working')"
          />
        </button>
      </template>
    </TransitionGroup>

    <LocaleToggle />
  </aside>
</template>
