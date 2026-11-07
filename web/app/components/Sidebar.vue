<script setup lang="ts">
// Left column: the wordmark with the language key beside it, the assistant roster
// (when assistants are configured), the new-session button, the card library
// (every deck, opened as the Library page), that assistant's
// sessions (grouped Today / Earlier) and, at the foot, the connection status.
// Below lg it is the drawer; at lg it is the desk's second sheet of paper
// (the card's edge lives on the <aside> in app.vue).
//
// Alignment: the column pads 12px; session rows add 12px more, so every
// line of text starts 24px in from the card's edge on both sides.
// The wordmark (RumuMark) breathes while a turn runs, goes hollow when the
// connection drops, and doubles as the way home: clicking it returns to the
// landing page, as does the new-session button — the first prompt typed there
// starts the session. The open session is a bordered sheet of paper on this
// one; the rest are plain lines with their last activity time.
import { computed, ref } from 'vue'
import { LOCALE_LABELS, SUPPORTED_LOCALES, locale, setLocale, t } from '../i18n'
import type { Locale } from '../i18n'
import {
  connected,
  deleteSession,
  goHome,
  isBusy,
  openLibrary,
  openSession,
  sortedSessions,
  state,
  viewAssistant,
} from '../lib/store'
import type { SessionInfo } from '../lib/types'
import { layout, openAssistantSettings, openWorkspaceSettings } from '../lib/layout'
import RumuCat from './brand/RumuCat.vue'
import RumuMark from './brand/RumuMark.vue'
import Icon from './ui/Icon.vue'
import Popover from './ui/Popover.vue'

/** The wordmark and the new-session button are both the way home; they close
 * the drawer too on narrow screens. */
function home(): void {
  goHome()
  layout.sidebarOpen = false
}

function library(): void {
  openLibrary()
  layout.sidebarOpen = false
}

function workspaces(): void {
  openWorkspaceSettings(state.workspaces[0]?.name ?? '')
  layout.sidebarOpen = false
}

function pickSession(id: string): void {
  openSession(id)
  layout.sidebarOpen = false
}

/** The roster: every assistant deck, plus any deck only old sessions still
 * name (gone from the roster, its sessions must stay reachable). */
const roster = computed<{ id: string; name: string }[]>(() => {
  const out = state.assistants.map((c) => ({ id: c.id, name: c.name }))
  const known = new Set(out.map((c) => c.id))
  for (const s of state.sessions) {
    if (!known.has(s.assistant)) {
      out.push({ id: s.assistant, name: s.assistant })
      known.add(s.assistant)
    }
  }
  if (!known.has(state.assistant)) out.push({ id: state.assistant, name: state.assistant })
  return out
})

/** Bring an assistant deck into view: its sessions list, and the next session
 * is its. An open session of another deck is left for the landing page. */
function pickAssistant(id: string): void {
  const leaving = state.sessionId && state.session?.assistant !== id
  viewAssistant(id)
  if (leaving) goHome()
}

/** Other sessions with a tool call waiting on the user's approval (the open
 *  one shows it in its composer). */
const waiting = computed(
  () => new Set(state.decisions.map((d) => d.session).filter((s) => s !== state.sessionId)),
)

/** The session whose action menu is asking for delete confirmation. The menu
 * opens on its action list every time, so a reopened menu never starts on the
 * destructive step. */
const pendingDelete = ref<string | null>(null)

function toggleMenu(toggle: () => void): void {
  pendingDelete.value = null
  toggle()
}

function askDelete(id: string): void {
  pendingDelete.value = id
}

function cancelDelete(): void {
  pendingDelete.value = null
}

/** Confirmed from a row's menu: delete it (landing home when it was open). */
function removeSession(id: string): void {
  if (id === state.sessionId) layout.sidebarOpen = false
  void deleteSession(id)
}

/** The confirm step's Delete button: delete, then close the menu. */
function confirmRemove(id: string, close: () => void): void {
  pendingDelete.value = null
  removeSession(id)
  close()
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
  | { kind: 'session'; key: string; s: SessionInfo; last: string }

const rows = computed<Row[]>(() => {
  const midnight = new Date().setHours(0, 0, 0, 0)
  const out: Row[] = []
  let group = ''
  for (const s of sortedSessions.value) {
    if (s.assistant !== state.assistant) continue
    const today = s.updated_at >= midnight
    const g = today ? t('chat.sidebar.today') : t('chat.sidebar.earlier')
    if (g !== group) {
      out.push({ kind: 'head', key: `head:${g}`, label: g })
      group = g
    }
    const at = new Date(s.updated_at)
    const last = today
      ? at.toLocaleTimeString(locale.value, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
      : at.toLocaleString(locale.value, {
          month: 'short',
          day: 'numeric',
          hour: '2-digit',
          minute: '2-digit',
          hourCycle: 'h23',
        })
    out.push({ kind: 'session', key: s.id, s, last })
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
          class="icon-btn h-7 w-auto min-w-7 px-1.5 text-micro leading-none font-bold"
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
          <Icon name="close" :size="14" />
        </button>
      </span>
    </div>

    <div class="mb-2 flex flex-none flex-wrap items-center gap-1 px-1.5">
      <div role="tablist" class="contents">
        <button
          v-for="assistant in roster"
          :key="assistant.id"
          type="button"
          role="tab"
          :aria-selected="assistant.id === state.assistant"
          class="flex items-center gap-1.5 rounded-full border py-0.5 pr-2.5 pl-1.5 text-meta transition-colors duration-200"
          :class="
            assistant.id === state.assistant
              ? 'border-line bg-surface-2 font-semibold text-ink shadow-soft'
              : 'border-transparent text-ink-3 hover:bg-surface-2/60 hover:text-ink'
          "
          @click="pickAssistant(assistant.id)"
        >
          <RumuCat :size="18" :paper="false" />
          {{ assistant.name }}
        </button>
      </div>
      <button
        type="button"
        class="icon-btn size-6 flex-none"
        :title="t('assistants.manage')"
        :aria-label="t('assistants.manage')"
        @click="openAssistantSettings()"
      >
        <Icon name="edit" :size="14" />
      </button>
    </div>

    <button
      class="flex w-full flex-none items-center gap-2.5 rounded-control border border-transparent py-1.5 pl-2.5 pr-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/60 hover:text-ink lg:pr-3"
      @click="home"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="plus" :size="16" />
      </span>
      {{ t('chat.sidebar.new') }}
    </button>

    <button
      class="mt-0.5 flex w-full flex-none items-center gap-2.5 rounded-control border border-transparent py-1.5 pl-2.5 pr-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/60 hover:text-ink lg:pr-3"
      @click="library"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="memory" :size="16" class="text-memory" />
      </span>
      <span class="flex-1">{{ t('library.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ state.cards.length }}</span>
    </button>

    <button
      class="mt-0.5 flex w-full flex-none items-center gap-2.5 rounded-control border border-transparent py-1.5 pl-2.5 pr-1.5 text-left text-secondary text-ink-2 transition-colors duration-200 hover:bg-surface-2/60 hover:text-ink lg:pr-3"
      @click="workspaces"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="mount" :size="16" class="text-tool" />
      </span>
      <span class="flex-1">{{ t('workspaces.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ state.workspaces.length }}</span>
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
        <div
          v-else
          class="group flex w-full min-w-0 items-center rounded-control border text-left text-secondary transition-[background-color,border-color,color,box-shadow] duration-200 has-[[aria-expanded=true]]:bg-surface-2/60 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-brand/25"
          :class="
            r.s.id === state.sessionId
              ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
              : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink'
          "
        >
          <button
            class="flex min-w-0 flex-1 items-center gap-2 py-2 pr-1 pl-3 text-left outline-none"
            :aria-current="r.s.id === state.sessionId ? 'page' : undefined"
            @click="pickSession(r.s.id)"
          >
            <span class="min-w-0 flex-1 truncate">{{ r.s.title || t('common.untitled') }}</span>
            <span
              v-if="waiting.has(r.s.id)"
              class="size-2 flex-none rounded-full bg-brand ring-3 ring-brand-soft"
              :title="t('chat.ask.waiting')"
            />
            <span
              v-else-if="r.s.id === state.sessionId && isBusy"
              class="dot-live size-1.5 flex-none rounded-full bg-brand"
              :title="t('common.working')"
            />
          </button>
          <Popover
            class="mr-1.5 flex-none"
            :label="pendingDelete === r.s.id ? t('chat.sidebar.delete') : t('chat.sidebar.actions')"
            placement="bottom-end"
            menu
            :role="pendingDelete === r.s.id ? 'dialog' : 'menu'"
          >
            <template #trigger="{ open, toggle }">
              <button
                class="icon-btn relative size-6 flex-none opacity-60 transition-opacity duration-150 before:absolute before:-inset-1.5 hover:bg-line-soft/70 hover:text-ink group-hover:opacity-100 focus-visible:opacity-100 aria-expanded:opacity-100"
                :aria-label="t('chat.sidebar.actions')"
                :title="t('chat.sidebar.actions')"
                aria-haspopup="menu"
                :aria-expanded="open"
                @click.stop="toggleMenu(toggle)"
              >
                <Icon name="more" :size="14" />
              </button>
            </template>
            <template #default="{ close }">
              <!-- stage one: last activity + the action list -->
              <template v-if="pendingDelete !== r.s.id">
                <p class="px-2.5 pt-1.5 pb-1 text-micro text-ink-3">
                  {{ t('chat.sidebar.lastActive', { at: r.last }) }}
                </p>
                <div class="mx-1 mb-1 h-px bg-line-soft/70" />
                <button
                  type="button"
                  role="menuitem"
                  class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-danger-soft hover:text-danger focus-visible:bg-danger-soft focus-visible:text-danger"
                  @click="askDelete(r.s.id)"
                >
                  <Icon name="trash" :size="14" />
                  {{ t('chat.sidebar.delete') }}
                </button>
              </template>
              <!-- stage two: confirmation, in place -->
              <div v-else class="px-2.5 py-2">
                <p class="text-secondary text-ink-2">{{ t('chat.sidebar.deletePrompt') }}</p>
                <p class="mt-1 text-micro text-ink-3">{{ t('chat.sidebar.deleteWarning') }}</p>
                <div class="mt-2 flex items-center justify-end gap-3">
                  <button
                    type="button"
                    class="text-micro font-semibold text-ink-2 transition-colors hover:text-ink"
                    @click="cancelDelete"
                  >
                    {{ t('common.cancel') }}
                  </button>
                  <button
                    type="button"
                    class="text-micro font-semibold text-danger transition-colors hover:underline"
                    @click="confirmRemove(r.s.id, close)"
                  >
                    {{ t('chat.sidebar.deleteConfirm') }}
                  </button>
                </div>
              </div>
            </template>
          </Popover>
        </div>
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
