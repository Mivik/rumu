<script setup lang="ts">
// Left column: the wordmark with the language key beside it, the landing,
// library, assistants and projects entries, and the sessions in groups (a
// two-level tree). By project, a session is in its project's group, or in
// its assistant's when it has none; by time, in the stretch it was last
// touched in, its project's chip on a second line. Sessions that need the
// user can sit in a section of their own on top. Each group carries its
// sessions, newest first, the newest few until "show more". Dropping a
// session on another project's group moves it there, on its assistant's
// group out of its project. At the foot, the connection status.
//
// Alignment: the column pads 8px; session rows add 12px more, so every line
// of text starts 20px in from the card's edge on both sides.
// The wordmark (RumuMark) breathes while a turn runs, goes hollow when the
// connection drops, and doubles as the way home: clicking it returns to the
// landing page, as does the new-session button. The open session is a
// bordered sheet of paper on this one; the rest are plain lines. Each row ends
// in its last activity time, which turns into the row's menu on hover.
import { computed, onUnmounted, onUpdated, ref, watch, type Directive } from 'vue'
import { LOCALE_LABELS, SUPPORTED_LOCALES, locale, setLocale, t } from '../i18n'
import type { Locale, MessageKey } from '../i18n'
import {
  connected,
  deleteSession,
  goHome,
  isBusy,
  isUnseen,
  sessionStatus,
  landingAssistant,
  landingProject,
  moveSession,
  openLibrary,
  openSession,
  renameSession,
  retitleSession,
  startSessionIn,
  state,
} from '../lib/store'
import {
  allSessions,
  compatible,
  machineLabel,
  machines,
  machineState,
  multiMachine,
  namesOf,
  switchMachine,
  type MachineName,
  type MachineSession,
} from '../lib/machines'
import { assistantCat } from '../lib/assistantFace'
import { mountedCardCount } from '../lib/cards'
import type { Project } from '../lib/types'
import { layout } from '../lib/layout'
import { copyAddress, sessionAddress } from '../lib/links'
import { openPage, sheetOf, type Page } from '../lib/router'
import { landingWritten } from '../lib/state/drafts'
import { models } from '../lib/state/models'
import { prefs } from '../lib/state/prefs'
import { requestComposerFocus } from '../lib/state/ui'
import { relTime } from '../lib/util'
import AssistantAvatar from './brand/AssistantAvatar.vue'
import ProjectSeal from './brand/ProjectSeal.vue'
import RumuCat from './brand/RumuCat.vue'
import RumuMark from './brand/RumuMark.vue'
import ConfirmPrompt from './ui/ConfirmPrompt.vue'
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

/** Start the next session in `group`: in its project, or as its assistant
 *  in none. The group unfolds so the unsent session's row is visible where
 *  it will be born. */
async function newIn(group: Group): Promise<void> {
  prefs.collapsedProjects[group.key] = false
  layout.sidebarOpen = false
  if (group.machine !== machines.current) await switchMachine(group.machine)
  startSessionIn(group.projectId, group.assistant ?? undefined)
}

/** Show another machine: its library, assistants and projects, and new
 *  sessions start there. One that speaks another protocol opens its own
 *  page instead. */
async function showMachine(name: MachineName, close: () => void): Promise<void> {
  close()
  const m = machineState(name)
  if (m && !compatible(m)) {
    window.open(m.url, '_blank', 'noopener')
    return
  }
  layout.sidebarOpen = false
  if (name === machines.current) return
  await switchMachine(name)
  goHome()
}

/** A machine's counts in the menu: running, and needing the user. */
function machineCounts(name: MachineName): { running: number; needs: number } {
  const list = sortedSessions.value.filter((s) => s.machine === name)
  return {
    running: list.filter((s) => sessionStatus(s) === 'running').length,
    needs: list.filter((s) => s.id !== state.sessionId && needsYou(s)).length,
  }
}

/** What the menu says of a machine it cannot show here. */
function machineNote(name: MachineName): string | null {
  const m = machineState(name)
  if (!m || !name) return null
  if (!m.connected) return t('chat.sidebar.machineOffline')
  if (m.protocol !== null && !compatible(m)) return t('chat.sidebar.machineMismatch')
  return null
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

/** A session row: any machine's (`s.machine`). */
type SessionInfo = MachineSession

interface SessionRow {
  s: SessionInfo
  /** Compact relative time, shown at the row's end. */
  ago: string
  /** The full time, in the row's menu. */
  last: string
}

/** One group of the tree: a project, an assistant's sessions in none, a
 *  stretch of time, or the sessions that need the user. */
interface Group {
  key: string
  kind: 'project' | 'assistant' | 'time' | 'attention'
  /** The project a dropped session moves into (`null`: out of its own). */
  projectId: string | null
  project: Project | null
  /** An assistant's group: whose sessions without a project it holds. */
  assistant: string | null
  /** The machine of a project's or assistant's group. */
  machine: MachineName
  name: string
  sessions: SessionRow[]
  activity: number
  /** The landing composer is a new session of this group that sending will
   *  create; until then it is only the intent. */
  draft: boolean
  /** Rows shown before "show more". */
  peek: number
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

/** Every machine's sessions, last touched first. */
const sortedSessions = computed(() => allSessions(state.sessions).sort((a, b) => b.touched_at - a.touched_at))

/** Is the session on the machine the page shows (its menu, dragging). */
function here(s: SessionInfo): boolean {
  return s.machine === machines.current
}

/** A group key for machine `m`'s project or assistant `id`: the serving
 *  machine's are the bare ids they were before there were machines. */
function keyOn(m: MachineName, id: string): string {
  return m ? `${m}/${id}` : id
}

/** Rows a group shows before "show more": a long project must not push the
 *  others out of reach. */
const PEEK = 5

type GroupInit = Omit<Group, 'key' | 'sessions' | 'activity' | 'draft'>

function toRow(s: SessionInfo): SessionRow {
  return { s, ago: relTime(s.touched_at, now.value), last: lastActive(s) }
}

/** Does the session need the user: a decision waiting on them, a turn that
 *  failed, or a finished one they have not seen. */
function needsYou(s: SessionInfo): boolean {
  const status = sessionStatus(s)
  return status === 'waiting' || status === 'failed' || (status === 'idle' && isUnseen(s))
}

/** The open session, when it needed the user as it was opened: it stays in
 *  their section until another is opened, so the row clicked there does not
 *  leave from under the pointer as the session is seen. */
const held = ref<string | null>(null)
watch(
  () => state.sessionId,
  (id) => {
    const s = id ? sortedSessions.value.find((x) => x.id === id) : undefined
    held.value = s && needsYou(s) ? s.id : null
  },
  { flush: 'sync', immediate: true },
)

/** The sessions in the section on top: the others that need the user, and
 *  the open one held there. The open one never enters it: what it needs is
 *  in its composer. */
const attention = computed(
  () =>
    new Set(
      prefs.attentionFirst
        ? sortedSessions.value
            .filter((s) => s.id === held.value || (s.id !== state.sessionId && needsYou(s)))
            .map((s) => s.id)
        : [],
    ),
)

/** By project: the groups with sessions, the latest activity first (projects
 *  and assistants alike), then the empty projects by name; sessions newest
 *  first. A session is in its project's group when this client knows the
 *  project (one a model just made shows once the list is refreshed, a
 *  deleted one never), else in its assistant's. A session in the section on
 *  top still counts to its group's activity, so the groups stay put. */
function byProject(): Group[] {
  const map = new Map<string, Group>()
  const group = (key: string, init: () => GroupInit): Group => {
    let g = map.get(key)
    if (!g) {
      g = { key, ...init(), sessions: [], activity: -Infinity, draft: false }
      map.set(key, g)
    }
    return g
  }
  const projectGroup = (p: Project, m: MachineName) =>
    group(keyOn(m, p.id), () => ({
      kind: 'project',
      projectId: p.id,
      project: p,
      assistant: null,
      machine: m,
      name: p.name,
      peek: PEEK,
    }))
  const assistantGroup = (id: string, m: MachineName) =>
    group(keyOn(m, id), () => ({
      kind: 'assistant',
      projectId: null,
      project: null,
      assistant: id,
      machine: m,
      name: assistantName(id, m),
      peek: PEEK,
    }))
  // Empty projects show for the shown machine only.
  for (const p of state.projects) projectGroup(p, machines.current)
  for (const s of sortedSessions.value) {
    const p = s.project ? projectOf(s) : null
    const g = p ? projectGroup(p, s.machine) : assistantGroup(s.assistant, s.machine)
    if (!attention.value.has(s.id)) g.sessions.push(toRow(s))
    g.activity = Math.max(g.activity, s.touched_at)
  }
  // The landing composer is a new session not created yet: its row sits in
  // the group it would be born into.
  if (draftShown()) {
    const into = landingProject.value
      ? map.get(keyOn(machines.current, landingProject.value))
      : landingAssistant.value
        ? assistantGroup(landingAssistant.value, machines.current)
        : undefined
    if (into) into.draft = true
  }
  return [...map.values()].sort((a, b) => {
    // The unsent session is not a reason to move a group: going home would
    // reshuffle the whole column under the pointer, and opening the session
    // shuffle it back. It lands where its group already stands.
    const busyA = a.activity > -Infinity
    const busyB = b.activity > -Infinity
    if (busyA !== busyB) return busyA ? -1 : 1
    if (busyA && a.activity !== b.activity) return b.activity - a.activity
    if (!a.projectId !== !b.projectId) return a.projectId ? -1 : 1
    return a.name.localeCompare(b.name, locale.value)
  })
}

/** The stretches of the time view, newest first: the start of each (local
 *  midnights), the recent ones shown whole. */
const STRETCHES = [
  { key: 'today', days: 0, peek: Infinity },
  { key: 'yesterday', days: 1, peek: Infinity },
  { key: 'week', days: 7, peek: PEEK * 2 },
  { key: 'month', days: 30, peek: PEEK * 2 },
  { key: 'earlier', days: Infinity, peek: PEEK * 2 },
] as const

/** By time: the stretches with sessions, newest first. The unsent session
 *  sits on top of today. */
function byTime(): Group[] {
  const midnight = new Date(now.value).setHours(0, 0, 0, 0)
  const groups: Group[] = STRETCHES.map((st) => ({
    key: `@${st.key}`,
    kind: 'time',
    projectId: null,
    project: null,
    assistant: null,
    machine: '',
    name: STRETCH_NAMES[st.key],
    peek: st.peek,
    sessions: [],
    activity: midnight - st.days * 86_400_000,
    draft: false,
  }))
  for (const s of sortedSessions.value) {
    if (attention.value.has(s.id)) continue
    groups.find((g) => s.touched_at >= g.activity)?.sessions.push(toRow(s))
  }
  groups[0]!.draft = draftShown()
  return groups.filter((g) => g.sessions.length || g.draft)
}

const STRETCH_NAMES = {
  today: 'chat.sidebar.today',
  yesterday: 'chat.sidebar.yesterday',
  week: 'chat.sidebar.week',
  month: 'chat.sidebar.month',
  earlier: 'chat.sidebar.earlier',
} as const satisfies Record<(typeof STRETCHES)[number]['key'], MessageKey>

/** The landing composer is a new session not created yet: it shows as a row
 *  unless a boot is still resolving. Once written it stays while other
 *  sessions are open, until sent or emptied. */
function draftShown(): boolean {
  return !state.booting && (onLanding.value || landingWritten.value)
}

/** The groups in view: the section of sessions that need the user on top,
 *  then the chosen grouping. */
const groups = computed<Group[]>(() => {
  const list = prefs.sidebarGrouping === 'time' ? byTime() : byProject()
  const needing = sortedSessions.value.filter((s) => attention.value.has(s.id))
  if (!needing.length) return list
  const top: Group = {
    key: '@attention',
    kind: 'attention',
    projectId: null,
    project: null,
    assistant: null,
    machine: '',
    name: 'chat.sidebar.attention',
    peek: Infinity,
    sessions: needing.map(toRow),
    activity: Infinity,
    draft: false,
  }
  return [top, ...list]
})

/** A group's name: a project's or assistant's own, else a message key. */
function groupName(group: Group): string {
  return group.kind === 'time' || group.kind === 'attention' ? t(group.name as MessageKey) : group.name
}

/** Rows of a group outside its project's group carry the project's chip. */
function chipped(group: Group, s: SessionInfo): boolean {
  return (group.kind === 'time' || group.kind === 'attention') && (!!s.project || !here(s))
}

/** Does a project's or assistant's group name its machine: one the page
 *  does not show. */
function elsewhereGroup(group: Group): boolean {
  return (
    multiMachine.value &&
    group.machine !== machines.current &&
    (group.kind === 'project' || group.kind === 'assistant')
  )
}

/** Does the row name its machine: one the page does not show. */
function elsewhere(s: SessionInfo): boolean {
  return multiMachine.value && !here(s)
}

function collapsed(group: Group): boolean {
  return !!prefs.collapsedProjects[group.key]
}

function toggleGroup(group: Group): void {
  prefs.collapsedProjects[group.key] = !collapsed(group)
}

/** Groups unfolded past their peek, for this page's life. */
const unfolded = ref(new Set<string>())

/** The newest rows up to the group's peek, plus any older one that is open
 *  or waiting on the user (hiding those would hide where the user is, or
 *  what needs them). */
function shown(group: Group): SessionRow[] {
  if (unfolded.value.has(group.key)) return group.sessions
  return group.sessions.filter(
    (row, i) => i < group.peek || row.s.id === state.sessionId || waiting.value.has(row.s.id),
  )
}

/** The project a session is in, as this client knows it. */
function projectOf(s: SessionInfo): Project | null {
  return s.project ? (namesOf(s.machine).projects.find((p) => p.id === s.project) ?? null) : null
}

/** A chip on a row's second line. */
const ROW_CHIP =
  'inline-flex h-[18px] min-w-0 max-w-[10rem] items-center gap-1 rounded-full border border-line-soft pr-1.5 pl-1'

const MENU_ITEM =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:bg-surface-2 focus-visible:text-ink'

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

function assistantName(id: string, m: MachineName = machines.current): string {
  return namesOf(m).assistants.find((a) => a.id === id)?.name ?? id.slice(0, 8)
}

/** An assistant group's face, from its machine's roster. */
function groupCat(group: Group) {
  const id = group.assistant!
  return assistantCat(namesOf(group.machine).assistants.find((a) => a.id === id) ?? { id })
}

/** Other sessions with a decision waiting on the user (the open one shows
 *  it in its composer). */
const waiting = computed(
  () =>
    new Set(
      sortedSessions.value.filter((s) => s.status === 'waiting' && s.id !== state.sessionId).map((s) => s.id),
    ),
)

type RowDot = 'waiting' | 'running' | 'failed' | 'background' | 'unseen'

/** The dot left of a row's title, one at a time: what the session is doing,
 *  else activity not seen yet. The open session's composer asks for its
 *  decisions, so there it reads as running. */
function rowDot(s: SessionInfo): RowDot | null {
  const status = sessionStatus(s)
  if (status === 'waiting') return s.id === state.sessionId ? 'running' : 'waiting'
  if (status !== 'idle') return status
  return s.id !== state.sessionId && isUnseen(s) ? 'unseen' : null
}

const ROW_DOT: Record<RowDot, { class: string; title: MessageKey }> = {
  waiting: { class: 'size-2 bg-brand ring-3 ring-brand-soft', title: 'chat.ask.waiting' },
  running: { class: 'dot-live dot-run size-1.5 bg-pine', title: 'common.working' },
  failed: { class: 'size-1.5 bg-danger', title: 'chat.sidebar.failed' },
  background: { class: 'size-1.5 bg-pine', title: 'chat.sidebar.background' },
  unseen: { class: 'size-1.5 bg-brand', title: 'chat.sidebar.unseen' },
}

/** Titles are the title model's to write, so a session's can be redone. */
const titledByModel = computed(() => models.settings?.titles === 'model' && !!models.settings.roles.title)

/** The session whose title is being edited in place, and the text in it. */
const renaming = ref<SessionInfo | null>(null)
const renameText = ref('')

function beginRename(session: SessionInfo): void {
  renaming.value = session
  renameText.value = session.title
}

/** Enter or leaving the field saves a changed, non-blank title; Escape drops
 *  it. Both close the field, which blurs it once more: `renaming` is null
 *  by then. */
function endRename(save: boolean): void {
  const session = renaming.value
  renaming.value = null
  const title = renameText.value.trim()
  if (save && session && title && title !== session.title) void renameSession(session.id, title)
}

function onRenameKey(e: KeyboardEvent): void {
  if (e.isComposing) return
  if (e.key === 'Enter') endRename(true)
  else if (e.key === 'Escape') {
    e.stopPropagation()
    endRename(false)
  }
}

/** Focus the field and select its text as it mounts. */
const vSelect: Directive<HTMLInputElement> = {
  mounted(el) {
    el.focus()
    el.select()
  },
}

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

/** May the dragged session drop on `group`? Another project's group, or its
 *  own assistant's when it is in a project (moving it out). */
function accepts(group: Group): boolean {
  const s = sortedSessions.value.find((x) => x.id === dragging.value)
  if (!s || group.machine !== s.machine) return false
  if (group.projectId) return group.projectId !== s.project
  return group.assistant === s.assistant && !!s.project
}

function overGroup(e: DragEvent, group: Group): void {
  if (!accepts(group)) return
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
  const ok = accepts(group)
  dragging.value = null
  dragOver.value = null
  if (id && ok) void moveSession(id, group.projectId)
}

function endDrag(): void {
  dragging.value = null
  dragOver.value = null
}

const presence = computed(() => {
  if (!connected.value)
    return { live: false, offline: true, title: t('common.offlineReconnecting'), dot: 'bg-danger' }
  if (isBusy.value) return { live: true, offline: false, title: t('common.working'), dot: 'bg-pine' }
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

    <!-- the list's heading, with how it is grouped -->
    <div class="mt-2 flex flex-none items-center border-t border-line-soft pt-1.5 pr-1.5 pl-3">
      <span class="flex-1 text-micro font-bold text-ink-3">{{ t('chat.sidebar.sessions') }}</span>
      <Popover :label="t('chat.sidebar.listView')" placement="bottom-end" menu role="menu">
        <template #trigger="{ open, toggle }">
          <button
            type="button"
            class="icon-btn size-6"
            :class="{ 'bg-surface-2 text-ink': open }"
            :title="t('chat.sidebar.listView')"
            :aria-label="t('chat.sidebar.listView')"
            aria-haspopup="menu"
            :aria-expanded="open"
            @click="toggle"
          >
            <Icon name="view" :size="14" />
          </button>
        </template>
        <template #default>
          <button
            v-for="g in ['project', 'time'] as const"
            :key="g"
            type="button"
            role="menuitemradio"
            :aria-checked="prefs.sidebarGrouping === g"
            :class="MENU_ITEM"
            @click="prefs.sidebarGrouping = g"
          >
            <span class="flex-1">{{
              t(g === 'project' ? 'chat.sidebar.byProject' : 'chat.sidebar.byTime')
            }}</span>
            <Icon name="ok" :size="14" :class="prefs.sidebarGrouping === g ? 'text-ink' : 'invisible'" />
          </button>
          <div class="mx-1 my-1 h-px bg-line-soft/70" />
          <button
            type="button"
            role="menuitemcheckbox"
            :aria-checked="prefs.attentionFirst"
            :class="MENU_ITEM"
            @click="prefs.attentionFirst = !prefs.attentionFirst"
          >
            <span class="flex-1">{{ t('chat.sidebar.attentionFirst') }}</span>
            <Icon name="ok" :size="14" :class="prefs.attentionFirst ? 'text-ink' : 'invisible'" />
          </button>
        </template>
      </Popover>
    </div>

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
            <AssistantAvatar
              v-else-if="group.assistant"
              :id="group.assistant"
              :cat="groupCat(group)"
              :size="16"
              class="mr-0.5"
            />
            <!-- another machine's group names it on a second line -->
            <span class="flex min-w-0 flex-col">
              <span class="truncate text-meta text-ink-2">{{ groupName(group) }}</span>
              <span v-if="elsewhereGroup(group)" class="mt-1 self-start font-normal" :class="ROW_CHIP">
                <Icon name="machine" :size="11" class="flex-none" />
                <span class="truncate">{{ machineLabel(group.machine) }}</span>
              </span>
            </span>
          </button>
          <div
            class="flex flex-none items-center gap-1 py-1.5 pr-1.5 transition-opacity duration-150 pointer-fine:opacity-0 pointer-fine:group-hover/head:opacity-100 pointer-fine:focus-within:opacity-100"
          >
            <button
              v-if="group.kind === 'project' || group.kind === 'assistant'"
              type="button"
              class="icon-btn size-6"
              :title="t('chat.sidebar.newSession')"
              :aria-label="t('chat.sidebar.newSession')"
              @click.stop="newIn(group)"
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
          class="flex w-full min-w-0 items-center rounded-control border py-2 pr-1 pl-5.5 text-left text-secondary transition-colors duration-200 outline-none focus-visible:ring-2 focus-visible:ring-brand/25"
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
            :draggable="here(row.s) && renaming?.id !== row.s.id"
            class="group relative flex w-full min-w-0 items-center rounded-control border text-left text-secondary transition-[background-color,border-color,color,box-shadow] duration-200 has-[[aria-expanded=true]]:bg-surface-2/60 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-brand/25"
            :class="
              row.s.id === state.sessionId && !state.page
                ? 'border-line bg-surface-2 font-bold text-ink shadow-soft'
                : 'border-transparent text-ink-2 hover:bg-surface-2/60 hover:text-ink'
            "
            @dragstart="startDrag($event, row.s)"
            @dragend="endDrag"
          >
            <span v-if="renaming?.id === row.s.id" class="flex min-w-0 flex-1 py-1 pr-2.5 pl-4">
              <input
                v-model="renameText"
                v-select
                type="text"
                class="w-full min-w-0 px-1.5 py-0.5 font-sans text-secondary font-normal"
                :aria-label="t('chat.sidebar.rename')"
                autocomplete="off"
                spellcheck="false"
                @keydown="onRenameKey"
                @blur="endRename(true)"
              />
            </span>
            <button
              v-else
              class="flex min-w-0 flex-1 flex-col py-2 pr-2.5 pl-2 text-left outline-none pointer-coarse:pr-10"
              :aria-current="row.s.id === state.sessionId && !state.page ? 'page' : undefined"
              @click="pickSession(row.s.id)"
            >
              <span class="flex w-full min-w-0 items-center gap-2">
                <!-- the dot's column, under the group's chevron: titles line up
                     whether a row has one or not -->
                <span class="-mr-0.5 flex size-2 flex-none items-center justify-center">
                  <span
                    v-if="rowDot(row.s)"
                    class="flex-none rounded-full"
                    :class="ROW_DOT[rowDot(row.s)!].class"
                    :title="t(ROW_DOT[rowDot(row.s)!].title)"
                  />
                </span>
                <span class="min-w-0 flex-1 truncate">{{ row.s.title || t('common.untitled') }}</span>
                <!-- the menu takes this place on hover -->
                <span
                  class="flex-none text-micro font-normal text-ink-3 tabular-nums transition-opacity duration-150 pointer-fine:group-hover:opacity-0 pointer-fine:group-has-[:focus-visible]:opacity-0 pointer-fine:group-has-[[aria-expanded=true]]:opacity-0"
                  >{{ row.ago }}</span
                >
              </span>
              <!-- away from its group, a row's second line names its machine
                   (another than the one shown) and its project -->
              <span
                v-if="chipped(group, row.s)"
                class="mt-1 flex min-w-0 items-center gap-1.5 pl-3.5 text-micro font-normal text-ink-3"
              >
                <span v-if="elsewhere(row.s)" :class="ROW_CHIP">
                  <Icon name="machine" :size="11" class="flex-none" />
                  <span class="truncate">{{ machineLabel(row.s.machine) }}</span>
                </span>
                <span v-if="row.s.project" :class="ROW_CHIP">
                  <ProjectSeal v-if="projectOf(row.s)" :project="projectOf(row.s)!" :size="12" />
                  <Icon v-else name="mount" :size="11" class="flex-none text-tool" />
                  <span class="truncate">{{ projectOf(row.s)?.name ?? row.s.project!.slice(0, 8) }}</span>
                </span>
              </span>
            </button>
            <span
              v-if="here(row.s)"
              class="absolute right-1.5 flex items-center"
              :class="chipped(group, row.s) ? 'top-2' : 'inset-y-0'"
            >
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
                      type="button"
                      role="menuitem"
                      class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:bg-surface-2 focus-visible:text-ink"
                      @click="(close(), beginRename(row.s))"
                    >
                      <Icon name="edit" :size="14" />
                      {{ t('chat.sidebar.rename') }}
                    </button>
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
                      class="flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink focus-visible:bg-surface-2 focus-visible:text-ink"
                      @click="(close(), copyAddress(sessionAddress(row.s.id)))"
                    >
                      <Icon name="link" :size="14" />
                      {{ t('chat.sidebar.copyAddress') }}
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
                  <ConfirmPrompt
                    v-else
                    :prompt="t('chat.sidebar.deletePrompt')"
                    :warning="t('chat.sidebar.deleteWarning')"
                    :confirm="t('chat.sidebar.deleteConfirm')"
                    @cancel="cancelDelete"
                    @confirm="confirmRemove(row.s.id, close)"
                  />
                </template>
              </Popover>
            </span>
          </div>
        </TransitionGroup>
        <button
          v-if="!collapsed(group) && group.sessions.length > group.peek"
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

    <!-- the connection, at the foot; with other machines, which one the
         page shows, and the menu to show another -->
    <div class="flex flex-none items-center gap-2 border-t border-line-soft px-3 py-3 text-meta text-ink-3">
      <Popover
        v-if="multiMachine"
        class="min-w-0 flex-1"
        :label="t('chat.sidebar.machines')"
        placement="top-start"
        menu
        role="menu"
        block
      >
        <template #trigger="{ open, toggle }">
          <button
            type="button"
            class="-mx-1.5 -my-1 flex min-w-0 flex-1 items-center gap-2 rounded-control px-1.5 py-1 text-left transition-colors duration-150 hover:bg-surface-2 hover:text-ink"
            :class="open ? 'bg-surface-2 text-ink' : 'text-ink-2'"
            :title="presence.title"
            aria-haspopup="menu"
            :aria-expanded="open"
            @click="toggle"
          >
            <span
              class="size-2 flex-none rounded-full"
              :class="[presence.dot, presence.live && 'dot-live dot-run']"
            />
            <Icon name="machine" :size="14" class="flex-none text-ink-3" />
            <span class="min-w-0 truncate font-semibold" :class="{ 'text-danger': presence.offline }">{{
              machineLabel(machines.current)
            }}</span>
            <Icon name="chev-up" :size="12" class="flex-none text-ink-3" />
          </button>
        </template>
        <template #default="{ close }">
          <p class="px-2.5 pt-1.5 pb-1 text-micro font-bold text-ink-3">{{ t('chat.sidebar.machines') }}</p>
          <button
            v-for="m in machines.all"
            :key="m.name"
            type="button"
            role="menuitemradio"
            :aria-checked="m.name === machines.current"
            :class="[MENU_ITEM, { 'opacity-55': !m.connected && !!m.name }]"
            class="disabled:cursor-default"
            :disabled="!m.connected && !!m.name"
            @click="showMachine(m.name, close)"
          >
            <Icon name="machine" :size="14" class="flex-none self-start pt-0.5 text-ink-3" />
            <span class="flex min-w-0 flex-1 flex-col">
              <span class="wrap-anywhere" :class="{ 'font-semibold': m.name === machines.current }">{{
                machineLabel(m.name)
              }}</span>
              <span v-if="machineNote(m.name)" class="text-micro text-ink-3">{{ machineNote(m.name) }}</span>
            </span>
            <span
              v-if="!machineNote(m.name)"
              class="flex flex-none items-center gap-2 text-micro tabular-nums text-ink-3"
            >
              <span v-if="machineCounts(m.name).running" class="flex items-center gap-1"
                ><span class="size-1.5 rounded-full bg-pine" />{{ machineCounts(m.name).running }}</span
              >
              <span v-if="machineCounts(m.name).needs" class="flex items-center gap-1"
                ><span class="size-1.5 rounded-full bg-brand" />{{ machineCounts(m.name).needs }}</span
              >
            </span>
            <Icon
              :name="m.protocol !== null && !compatible(m) ? 'external' : 'ok'"
              :size="14"
              class="flex-none"
              :class="
                m.name === machines.current || (m.protocol !== null && !compatible(m))
                  ? 'text-ink'
                  : 'invisible'
              "
            />
          </button>
        </template>
      </Popover>
      <template v-else>
        <span
          class="size-2 flex-none rounded-full"
          :class="[presence.dot, presence.live && 'dot-live dot-run']"
        />
        <span
          class="flex-1 first-letter:uppercase"
          :class="{ 'font-semibold text-danger': presence.offline }"
          >{{ presence.title }}</span
        >
      </template>
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
