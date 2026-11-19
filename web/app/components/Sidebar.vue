<script setup lang="ts">
// Left column: the wordmark with the language key beside it, the landing,
// library, assistants and projects entries, and the sessions grouped by
// project (a two-level tree). Each project group carries its sessions, newest
// first, the newest few until "show more"; the last group holds the sessions
// without a project. A drag between groups moves the session. At the foot,
// the connection status.
//
// Alignment: the column pads 8px; session rows add 12px more, so every line
// of text starts 20px in from the card's edge on both sides.
// The wordmark (RumuMark) breathes while a turn runs, goes hollow when the
// connection drops, and doubles as the way home: clicking it returns to the
// landing page, as does the new-session button. The open session is a
// bordered sheet of paper on this one; the rest are plain lines. Each row ends
// in its last activity time, which turns into the row's menu on hover.
import { computed, onUnmounted, onUpdated, ref, watch } from 'vue'
import { LOCALE_LABELS, SUPPORTED_LOCALES, locale, setLocale, t } from '../i18n'
import type { Locale } from '../i18n'
import {
  connected,
  deleteSession,
  goHome,
  isBusy,
  isUnseen,
  landingProject,
  moveSession,
  openLibrary,
  openSession,
  retitleSession,
  sortedSessions,
  startSessionIn,
  state,
} from '../lib/store'
import { mountedCardCount } from '../lib/cards'
import type { Project, SessionInfo } from '../lib/types'
import { layout } from '../lib/layout'
import { openPage, sheetOf, type Page } from '../lib/router'
import { landingWritten } from '../lib/state/drafts'
import { models } from '../lib/state/models'
import { prefs } from '../lib/state/prefs'
import { requestComposerFocus } from '../lib/state/ui'
import { relTime } from '../lib/util'
import ProjectSeal from './brand/ProjectSeal.vue'
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

/** Go to a page's list (the library loads its decks on the way). */
function page(name: Page): void {
  openPage(name)
  layout.sidebarOpen = false
}

/** The page open in the chat's place, which its entry here marks. */
const onPage = computed(() => (state.page ? sheetOf(state.page.page) : null))

/** The landing page is in view: its new session is the current row. */
const onLanding = computed(() => !state.sessionId && !state.page)

/** An entry under the wordmark: a sheet of paper while its page is open. */
function navTone(name: Page | null): string[] {
  return [
    'flex w-full flex-none items-center gap-2.5 rounded-control border py-1.5 pl-2.5 pr-1.5 text-left text-secondary transition-colors duration-200 lg:pr-3',
    name && onPage.value === name
      ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
      : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink',
  ]
}

function pickSession(id: string): void {
  openSession(id)
  layout.sidebarOpen = false
}

/** Start the next session in `project` (`null`: none). The group unfolds so
 *  the unsent session's row is visible where it will be born. */
function newIn(project: string | null): void {
  prefs.collapsedProjects[project ?? ''] = false
  startSessionIn(project)
  layout.sidebarOpen = false
}

/** The unsent new session is the landing page's composer: go back to it, or
 *  put the caret there when it is open already. */
function openDraft(): void {
  if (onLanding.value) requestComposerFocus()
  else goHome()
  layout.sidebarOpen = false
}

function projectSettings(project: Project): void {
  openPage('projects', project.id)
  layout.sidebarOpen = false
}

function openGroupSettings(group: Group): void {
  if (group.project) projectSettings(group.project)
}

interface SessionRow {
  s: SessionInfo
  /** Compact relative time, shown at the row's end. */
  ago: string
  /** The full time, in the row's menu. */
  last: string
}

/** One group of the tree: a project, or the sessions without one. */
interface Group {
  key: string
  /** The project a dropped session moves into (`null`: no project). */
  projectId: string | null
  project: Project | null
  name: string
  sessions: SessionRow[]
  activity: number
  /** The landing composer is a new session of this group that sending will
   *  create; until then it is only the intent. */
  draft: boolean
}

/** How a session row's menu names its last activity. */
function lastActive(s: SessionInfo): string {
  const midnight = new Date().setHours(0, 0, 0, 0)
  const at = new Date(s.updated_at)
  return s.updated_at >= midnight
    ? at.toLocaleTimeString(locale.value, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' })
    : at.toLocaleString(locale.value, {
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
        hourCycle: 'h23',
      })
}

/** The projects, those with recent activity first and the empty ones by
 *  name, then the group without a project; sessions newest first. */
const groups = computed<Group[]>(() => {
  const map = new Map<string, Group>()
  for (const p of state.projects)
    map.set(p.id, {
      key: p.id,
      projectId: p.id,
      project: p,
      name: p.name,
      sessions: [],
      activity: -Infinity,
      draft: false,
    })
  map.set('', {
    key: '',
    projectId: null,
    project: null,
    name: t('chat.sidebar.noProject'),
    sessions: [],
    activity: -Infinity,
    draft: false,
  })
  for (const s of sortedSessions.value) {
    const key = s.project ?? ''
    let group = map.get(key)
    // A project this client has not heard of yet (a model just made it).
    if (!group) {
      group = {
        key,
        projectId: key,
        project: null,
        name: key,
        sessions: [],
        activity: -Infinity,
        draft: false,
      }
      map.set(key, group)
    }
    group.sessions.push({ s, ago: relTime(s.touched_at, now.value), last: lastActive(s) })
    group.activity = Math.max(group.activity, s.touched_at)
  }
  // The landing composer is a new session not created yet: its row sits in
  // the group it would be born into, unless a boot is still resolving. Once
  // written it stays while other sessions are open, until sent or emptied.
  if (!state.booting && (onLanding.value || landingWritten.value)) {
    const draft = map.get(landingProject.value ?? '')
    if (draft) draft.draft = true
  }
  return [...map.values()].sort((a, b) => {
    if (!a.projectId !== !b.projectId) return a.projectId ? -1 : 1
    // The unsent session is not a reason to move a group: going home would
    // reshuffle the whole column under the pointer, and opening the session
    // shuffle it back. It lands where its group already stands.
    const busyA = a.sessions.length > 0
    const busyB = b.sessions.length > 0
    if (busyA !== busyB) return busyA ? -1 : 1
    if (busyA && a.activity !== b.activity) return b.activity - a.activity
    return a.name.localeCompare(b.name, locale.value)
  })
})

function collapsed(group: Group): boolean {
  return !!prefs.collapsedProjects[group.key]
}

function toggleGroup(group: Group): void {
  prefs.collapsedProjects[group.key] = !collapsed(group)
}

/** Rows a group shows before "show more": a long project must not push the
 *  others out of reach. */
const PEEK = 5

/** Groups unfolded past PEEK, for this page's life. */
const unfolded = ref(new Set<string>())

/** The newest PEEK rows, plus any older one that is open or waiting on the
 *  user (hiding those would hide where the user is, or what needs them). */
function shown(group: Group): SessionRow[] {
  if (unfolded.value.has(group.key)) return group.sessions
  return group.sessions.filter(
    (row, i) => i < PEEK || row.s.id === state.sessionId || waiting.value.has(row.s.id),
  )
}

function toggleMore(group: Group): void {
  const set = unfolded.value
  if (!set.delete(group.key)) set.add(group.key)
}

/** Groups and rows slide only in the render that reorders them. Vue restarts
 *  every running slide on any re-render of its group, and a prompt brings
 *  several (the create reply, pushes, the title) within one slide; each
 *  restart showed as the column jumping back and forth. The first fill of
 *  the list on boot is not a move either. */
const SLIDE = 'transition-transform duration-300 ease-soft'
const moveClass = ref('no-slide')
const order = computed(() =>
  groups.value.map((g) => [g.key, ...shown(g).map((row) => row.s.id)].join(',')).join('|'),
)
watch(order, (_, before) => (moveClass.value = before.includes(',') ? SLIDE : 'no-slide'), { flush: 'pre' })
onUpdated(() => (moveClass.value = 'no-slide'))

/** Ticks the rows' relative times. */
const now = ref(Date.now())
const clock = setInterval(() => (now.value = Date.now()), 30_000)
onUnmounted(() => clearInterval(clock))

/** The default assistant's sessions carry no assistant label. */
const defaultAssistant = computed(() => state.assistants.find((a) => a.default)?.id ?? '')

function assistantName(id: string): string {
  return state.assistants.find((a) => a.id === id)?.name ?? id.slice(0, 8)
}

/** Other sessions with a tool call waiting on the user's approval (the open
 *  one shows it in its composer). */
const waiting = computed(
  () => new Set(state.decisions.map((d) => d.session).filter((s) => s !== state.sessionId)),
)

/** Titles are the title model's to write, so a session's can be redone. */
const titledByModel = computed(() => models.settings?.titles === 'model' && !!models.settings.roles.title)

/** The session whose action menu is asking for delete confirmation. The menu
 *  opens on its action list every time, so a reopened menu never starts on the
 *  destructive step. */
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

// ---- dragging a session onto another group ----

const dragging = ref<string | null>(null)
const dragOver = ref<string | null>(null)

function startDrag(e: DragEvent, s: SessionInfo): void {
  dragging.value = s.id
  e.dataTransfer?.setData('text/plain', s.id)
  if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move'
}

function overGroup(e: DragEvent, group: Group): void {
  if (!dragging.value) return
  e.preventDefault()
  dragOver.value = group.key
}

function leaveGroup(e: DragEvent, group: Group): void {
  const to = e.relatedTarget as Node | null
  if (to && (e.currentTarget as Node).contains(to)) return
  if (dragOver.value === group.key) dragOver.value = null
}

function dropOn(e: DragEvent, group: Group): void {
  e.preventDefault()
  const id = dragging.value
  dragging.value = null
  dragOver.value = null
  if (!id) return
  const session = state.sessions.find((s) => s.id === id)
  if (!session || (session.project ?? null) === group.projectId) return
  void moveSession(id, group.projectId)
}

function endDrag(): void {
  dragging.value = null
  dragOver.value = null
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
</script>

<template>
  <aside class="flex min-h-0 min-w-0 flex-col px-2">
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

    <button :class="navTone(null)" @click="home">
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="plus" :size="16" />
      </span>
      {{ t('chat.sidebar.new') }}
    </button>

    <button
      class="mt-0.5"
      :class="navTone('decks')"
      :aria-current="onPage === 'decks' ? 'page' : undefined"
      @click="library"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="memory" :size="16" class="text-memory" />
      </span>
      <span class="flex-1">{{ t('library.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ mountedCardCount }}</span>
    </button>

    <button
      class="mt-0.5"
      :class="navTone('assistants')"
      :aria-current="onPage === 'assistants' ? 'page' : undefined"
      @click="page('assistants')"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <RumuCat :size="18" :paper="false" />
      </span>
      <span class="flex-1">{{ t('assistants.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ state.assistants.length }}</span>
    </button>

    <button
      class="mt-0.5"
      :class="navTone('projects')"
      :aria-current="onPage === 'projects' ? 'page' : undefined"
      @click="page('projects')"
    >
      <span class="flex size-5 flex-none items-center justify-center leading-0">
        <Icon name="mount" :size="16" class="text-tool" />
      </span>
      <span class="flex-1">{{ t('projects.title') }}</span>
      <span class="text-micro text-ink-3 tabular-nums">{{ state.projects.length }}</span>
    </button>

    <div class="mt-2 h-px flex-none bg-line-soft" />

    <!-- Groups slide as wholes when their order changes; otherwise each group's
         rows see only their own displacement and fly across the column. -->
    <TransitionGroup
      tag="div"
      class="scroll-fade -mx-2 flex flex-1 flex-col gap-1 overflow-y-auto px-2 pt-2 pb-3"
      :move-class="moveClass"
    >
      <section
        v-for="group in groups"
        :key="group.key"
        class="flex flex-col gap-0.5 rounded-control transition-colors duration-150"
        :class="{ 'bg-brand-soft/40 ring-1 ring-brand/25': dragOver === group.key }"
        @dragover="overGroup($event, group)"
        @dragleave="leaveGroup($event, group)"
        @drop="dropOn($event, group)"
      >
        <div
          class="group/head flex items-stretch rounded-control transition-colors duration-150 hover:bg-surface-2/60"
        >
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center gap-1 py-1.5 pl-1.5 text-left text-micro font-bold text-ink-3"
            :aria-expanded="!collapsed(group)"
            @click="toggleGroup(group)"
          >
            <Icon
              name="chevron"
              :size="12"
              class="flex-none transition-transform duration-150"
              :class="{ 'rotate-90': !collapsed(group) }"
            />
            <ProjectSeal v-if="group.project" :project="group.project" :size="16" class="mr-0.5" />
            <span class="min-w-0 truncate text-meta text-ink-2">{{ group.name }}</span>
            <span v-if="group.sessions.length" class="text-micro text-ink-3 tabular-nums">{{
              group.sessions.length
            }}</span>
          </button>
          <div
            class="flex flex-none items-center gap-1 py-1.5 pr-1.5 transition-opacity duration-150 pointer-fine:opacity-0 pointer-fine:group-hover/head:opacity-100 pointer-fine:focus-within:opacity-100"
          >
            <button
              type="button"
              class="icon-btn size-6"
              :title="t('chat.sidebar.newSession')"
              :aria-label="t('chat.sidebar.newSession')"
              @click.stop="newIn(group.projectId)"
            >
              <Icon name="plus" :size="14" />
            </button>
            <button
              v-if="group.project"
              type="button"
              class="icon-btn size-6"
              :title="t('chat.sidebar.projectSettings')"
              :aria-label="t('chat.sidebar.projectSettings')"
              @click.stop="openGroupSettings(group)"
            >
              <Icon name="settings" :size="13" />
            </button>
          </div>
        </div>

        <!-- the unsent new session: a row now, a session once a prompt is sent -->
        <button
          v-if="group.draft && !collapsed(group)"
          type="button"
          :aria-current="onLanding ? 'page' : undefined"
          class="flex w-full min-w-0 items-center rounded-control border py-2 pr-1 pl-3 text-left text-secondary transition-colors duration-200 outline-none focus-visible:ring-2 focus-visible:ring-brand/25"
          :class="
            onLanding
              ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
              : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink'
          "
          @click="openDraft"
        >
          <span class="min-w-0 flex-1 truncate">{{ t('chat.sidebar.newSession') }}</span>
        </button>

        <TransitionGroup
          v-if="!collapsed(group)"
          tag="div"
          class="flex flex-col gap-0.5 pb-1"
          :move-class="moveClass"
          enter-active-class="transition duration-300 ease-soft"
          enter-from-class="-translate-y-1 opacity-0"
        >
          <div
            v-for="row in shown(group)"
            :key="row.s.id"
            draggable="true"
            class="group relative flex w-full min-w-0 items-center rounded-control border text-left text-secondary transition-[background-color,border-color,color,box-shadow] duration-200 has-[[aria-expanded=true]]:bg-surface-2/60 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-brand/25"
            :class="
              row.s.id === state.sessionId && !state.page
                ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
                : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink'
            "
            @dragstart="startDrag($event, row.s)"
            @dragend="endDrag"
          >
            <button
              class="flex min-w-0 flex-1 items-center gap-2 py-2 pr-2.5 pl-3 text-left outline-none pointer-coarse:pr-10"
              :aria-current="row.s.id === state.sessionId && !state.page ? 'page' : undefined"
              @click="pickSession(row.s.id)"
            >
              <span class="min-w-0 flex-1 truncate">{{ row.s.title || t('common.untitled') }}</span>
              <span
                v-if="row.s.assistant !== defaultAssistant"
                class="max-w-[6.5rem] flex-none truncate text-micro font-normal text-ink-3"
                >{{ assistantName(row.s.assistant) }}</span
              >
              <span
                v-if="waiting.has(row.s.id)"
                class="size-2 flex-none rounded-full bg-brand ring-3 ring-brand-soft"
                :title="t('chat.ask.waiting')"
              />
              <span
                v-else-if="row.s.id === state.sessionId && isBusy"
                class="dot-live size-1.5 flex-none rounded-full bg-brand"
                :title="t('common.working')"
              />
              <span
                v-else-if="row.s.id !== state.sessionId && isUnseen(row.s)"
                class="size-1.5 flex-none rounded-full bg-brand"
                :title="t('chat.sidebar.unseen')"
              />
              <!-- the menu takes this place on hover -->
              <span
                class="flex-none text-micro font-normal text-ink-3 tabular-nums transition-opacity duration-150 pointer-fine:group-hover:opacity-0 pointer-fine:group-has-[:focus-visible]:opacity-0 pointer-fine:group-has-[[aria-expanded=true]]:opacity-0"
                >{{ row.ago }}</span
              >
            </button>
            <span class="absolute inset-y-0 right-1.5 flex items-center">
              <Popover
                :label="pendingDelete === row.s.id ? t('chat.sidebar.delete') : t('chat.sidebar.actions')"
                placement="bottom-end"
                menu
                :role="pendingDelete === row.s.id ? 'dialog' : 'menu'"
              >
                <template #trigger="{ open, toggle }">
                  <button
                    class="icon-btn relative size-6 flex-none transition-opacity duration-150 before:absolute before:-inset-1.5 hover:bg-line-soft/70 hover:text-ink pointer-fine:opacity-0 pointer-fine:group-hover:opacity-100 pointer-fine:group-has-[:focus-visible]:opacity-100 pointer-fine:aria-expanded:opacity-100"
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
                  <template v-if="pendingDelete !== row.s.id">
                    <p class="px-2.5 pt-1.5 pb-1 text-micro text-ink-3">
                      {{ t('chat.sidebar.lastActive', { at: row.last }) }}
                    </p>
                    <div class="mx-1 mb-1 h-px bg-line-soft/70" />
                    <button
                      v-if="titledByModel"
                      type="button"
                      role="menuitem"
                      class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:bg-surface-2 focus-visible:text-ink"
                      @click="(close(), retitleSession(row.s.id))"
                    >
                      <Icon name="regenerate" :size="14" />
                      {{ t('chat.sidebar.retitle') }}
                    </button>
                    <button
                      type="button"
                      role="menuitem"
                      class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-danger-soft hover:text-danger focus-visible:bg-danger-soft focus-visible:text-danger"
                      @click="askDelete(row.s.id)"
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
                        @click="confirmRemove(row.s.id, close)"
                      >
                        {{ t('chat.sidebar.deleteConfirm') }}
                      </button>
                    </div>
                  </div>
                </template>
              </Popover>
            </span>
          </div>
        </TransitionGroup>
        <button
          v-if="!collapsed(group) && group.sessions.length > PEEK"
          type="button"
          class="-mt-1 mb-1 self-start rounded-control py-1 pl-3 pr-2 text-micro text-ink-3 transition-colors duration-150 hover:text-ink-2"
          @click="toggleMore(group)"
        >
          {{
            unfolded.has(group.key)
              ? t('chat.sidebar.showLess')
              : t('chat.sidebar.showMore', { n: group.sessions.length - shown(group).length })
          }}
        </button>
      </section>
    </TransitionGroup>

    <!-- the connection, at the foot -->
    <div class="flex flex-none items-center gap-2 border-t border-line-soft px-3 py-3 text-meta text-ink-3">
      <span class="size-2 flex-none rounded-full" :class="[presence.dot, { 'dot-live': presence.live }]" />
      <span
        class="flex-1 first-letter:uppercase"
        :class="{ 'font-semibold text-danger': presence.offline }"
        >{{ presence.title }}</span
      >
      <button
        type="button"
        class="icon-btn -my-1 size-7 flex-none"
        :class="{ 'bg-surface-2 text-ink': onPage === 'settings' }"
        :title="t('settings.title')"
        :aria-label="t('settings.title')"
        :aria-current="onPage === 'settings' ? 'page' : undefined"
        @click="page('settings')"
      >
        <Icon name="settings" :size="15" />
      </button>
    </div>
  </aside>
</template>
