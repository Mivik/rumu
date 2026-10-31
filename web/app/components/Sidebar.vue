<script setup lang="ts">
// Left column: the wordmark with the language key beside it, the new-session
// button, the card library (rumu's memory, opened as the CardCatalog), the
// sessions (grouped Today / Earlier) and, at the foot, the connection status.
// Below lg it is the drawer; at lg it is the desk's second sheet of paper
// (the card's edge lives on the <aside> in app.vue).
//
// Alignment: the column pads 12px; rows and buttons add 12px more, so every
// line of text starts 24px in from the card's edge on both sides.
// The wordmark (RumuMark) breathes while a turn runs, goes hollow when the
// connection drops, and doubles as the way home: clicking it returns to the
// landing page, as does the new-session button — the first prompt typed there
// starts the session. The open session is a bordered sheet of paper on this
// one; the rest are plain lines with their last activity time.
import { computed } from 'vue'
import { LOCALE_LABELS, SUPPORTED_LOCALES, locale, setLocale, t } from '../i18n'
import type { Locale } from '../i18n'
import { connected, goHome, isBusy, openCatalog, openSession, sortedSessions, state } from '../lib/store'
import type { SessionInfo } from '../lib/types'
import { layout } from '../lib/layout'
import RumuMark from './brand/RumuMark.vue'
import Icon from './ui/Icon.vue'

/** The wordmark and the new-session button are both the way home; they close
 * the drawer too on narrow screens. */
function home(): void {
  goHome()
  layout.sidebarOpen = false
}

function library(): void {
  openCatalog()
  layout.sidebarOpen = false
}

function pickSession(id: string): void {
  openSession(id)
  layout.sidebarOpen = false
}

const presence = computed(() => {
  if (!connected.value)
    return { live: false, offline: true, title: t('common.offlineReconnecting'), dot: 'bg-danger' }
  if (isBusy.value) return { live: true, offline: false, title: t('common.working'), dot: 'bg-brand' }
  return { live: false, offline: false, title: t('common.connected'), dot: 'bg-pine' }
})

/** Two locales: the key names the one it switches to. */
const LOCALE_KEYS: Record<Locale, string> = { en: 'EN', 'zh-CN': '中' }
const otherLocale = computed<Locale>(
  () => SUPPORTED_LOCALES.find((l) => l !== locale.value) ?? SUPPORTED_LOCALES[0],
)

/** Flat list with group headers mixed in, so one TransitionGroup animates both. */
type Row =
  | { kind: 'head'; key: string; label: string }
  | { kind: 'session'; key: string; s: SessionInfo; when: string }

const rows = computed<Row[]>(() => {
  const midnight = new Date().setHours(0, 0, 0, 0)
  const out: Row[] = []
  let group = ''
  for (const s of sortedSessions.value) {
    const today = s.updated_at >= midnight
    const g = today ? t('chat.sidebar.today') : t('chat.sidebar.earlier')
    if (g !== group) {
      out.push({ kind: 'head', key: `head:${g}`, label: g })
      group = g
    }
    const at = new Date(s.updated_at)
    const when = today
      ? at.toLocaleTimeString(locale.value, { hour: '2-digit', minute: '2-digit' })
      : at.toLocaleDateString(locale.value, { month: 'short', day: 'numeric' })
    out.push({ kind: 'session', key: s.id, s, when })
  }
  return out
})
</script>

<template>
  <aside class="flex min-h-0 min-w-0 flex-col px-3">
    <div class="flex h-16 flex-none items-center gap-2 pl-3 pr-1.5 lg:pr-3">
      <button
        class="flex items-center rounded-md transition duration-200 ease-soft hover:opacity-70 active:scale-[.97]"
        :title="presence.title"
        :aria-label="t('chat.sidebar.home')"
        @click="home"
      >
        <RumuMark class="text-ink" :height="22" :live="presence.live" :offline="presence.offline" />
      </button>
      <span class="ml-auto flex items-center gap-1">
        <button
          type="button"
          class="icon-btn h-7 w-auto min-w-7 px-1.5 text-micro font-bold"
          :title="LOCALE_LABELS[otherLocale]"
          :aria-label="LOCALE_LABELS[otherLocale]"
          @click="setLocale(otherLocale)"
        >
          {{ LOCALE_KEYS[otherLocale] }}
        </button>
        <button
          class="icon-btn -mr-2 size-8 lg:hidden"
          :title="t('chat.sidebar.close')"
          :aria-label="t('chat.sidebar.close')"
          @click="layout.sidebarOpen = false"
        >
          <Icon name="close" :size="13" />
        </button>
      </span>
    </div>

    <button
      class="group/new flex w-full flex-none items-center gap-2.5 rounded-control border border-transparent py-2 pl-3 pr-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/60 hover:text-ink lg:pr-3"
      @click="home"
    >
      <span
        class="flex size-5 flex-none items-center justify-center rounded-full bg-brand text-on-brand transition-transform duration-300 ease-soft group-hover/new:rotate-90"
      >
        <Icon name="plus" :size="12" :stroke="2.6" />
      </span>
      {{ t('chat.sidebar.new') }}
    </button>

    <button
      class="mt-1.5 flex w-full flex-none items-center gap-2.5 rounded-control border border-transparent py-2 pl-3 pr-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/60 hover:text-ink lg:pr-3"
      @click="library"
    >
      <span class="flex size-5 flex-none items-center justify-center">
        <Icon name="memory" :size="16" class="text-memory" />
      </span>
      <span class="flex-1">{{ t('cards.library.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ state.cards.length }}</span>
    </button>

    <TransitionGroup
      tag="div"
      class="scroll-fade -mx-3 mt-2 flex flex-1 flex-col gap-0.5 overflow-y-auto px-3 pt-2 pb-3"
      move-class="transition-transform duration-300 ease-soft"
      enter-active-class="transition duration-300 ease-soft"
      enter-from-class="-translate-y-1 opacity-0"
    >
      <template v-for="r in rows" :key="r.key">
        <div v-if="r.kind === 'head'" class="px-3 pt-3 pb-1 text-micro font-bold text-ink-3">
          {{ r.label }}
        </div>
        <button
          v-else
          class="flex w-full min-w-0 items-center gap-2 rounded-control border py-2 pl-3 pr-1.5 text-left text-secondary transition-[background-color,border-color,color,box-shadow] duration-200 lg:pr-3"
          :class="
            r.s.id === state.sessionId
              ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
              : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink'
          "
          :aria-current="r.s.id === state.sessionId ? 'page' : undefined"
          @click="pickSession(r.s.id)"
        >
          <span class="min-w-0 flex-1 truncate">{{ r.s.title || t('common.untitled') }}</span>
          <span
            v-if="r.s.id === state.sessionId && isBusy"
            class="dot-live size-1.5 flex-none rounded-full bg-brand"
            :title="t('common.working')"
          />
          <span v-else class="flex-none text-micro font-normal text-ink-3 tabular-nums">{{ r.when }}</span>
        </button>
      </template>
    </TransitionGroup>

    <!-- the connection, at the foot -->
    <div class="flex flex-none items-center gap-2 border-t border-line-soft px-3 py-3 text-meta text-ink-3">
      <span class="size-2 flex-none rounded-full" :class="[presence.dot, { 'dot-live': presence.live }]" />
      <span class="first-letter:uppercase" :class="{ 'font-semibold text-danger': presence.offline }">{{
        presence.title
      }}</span>
    </div>
  </aside>
</template>
