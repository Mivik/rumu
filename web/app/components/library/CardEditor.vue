<script setup lang="ts">
// One card pane, shared by the library and the chat's card drawer: a reader
// by default, a form when editing (or for a new card), and the version
// history in place of either.
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { parse as parseYaml } from 'yaml'
import { t } from '../../i18n'
import { RpcError } from '../../lib/api'
import { formOfView, viewOfForm, type CardForm, type CardView } from '../../lib/cards'
import { diffLines } from '../../lib/diff'
import { renderMarkdown } from '../../lib/markdown'
import {
  deleteCard,
  loadCard,
  openCardWindow,
  openLibrary,
  refreshLibrary,
  saveCard,
  state,
} from '../../lib/store'
import { toast } from '../../lib/state/ui'
import type { LibraryCardDetail } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'
import Select from '../ui/Select.vue'
import type { SelectOption } from '../ui/Select.vue'
import CardFace from '../CardFace.vue'
import CardHistory from './CardHistory.vue'

type Mode = 'read' | 'edit'

const props = defineProps<{
  deck: string
  path: string
  isNew?: boolean
  /** Hosted by the library page (no "open in library" for itself). */
  inLibrary: boolean
  /** A read-only view to show instead of the loaded card (the drawer's
   *  "as seen" snapshot). Its presence hides edit. */
  override?: CardView | null
  /** Teleport target for the action menu; a native dialog passes itself. */
  overlay?: string
}>()

const emit = defineEmits<{ close: []; moved: [deck: string, path: string] }>()

/** A card or folder name: the path segment rule of the server. */
const ENTRY_NAME = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/

const MENU_ITEM =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink'
const MENU_DANGER =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-danger transition-colors duration-150 hover:bg-danger-soft'

function blankForm(): CardForm {
  return { description: '', keywords: [], pinned: false, extra: '', body: '' }
}

const detail = ref<LibraryCardDetail | null>(null)
const loading = ref(false)
const mode = ref<Mode>('read')
const deckId = ref('')
/** The card's location, edited as a folder and a name. */
const folder = ref('')
const name = ref('')
const path = computed({
  get: () => {
    const leaf = name.value.trim()
    return folder.value && leaf ? `${folder.value}/${leaf}` : leaf
  },
  set: (value: string) => {
    const at = value.lastIndexOf('/')
    folder.value = at < 0 ? '' : value.slice(0, at)
    name.value = at < 0 ? value : value.slice(at + 1)
  },
})
const form = reactive<CardForm>(blankForm())
const pristine = ref('')
const base = ref('')
const bodyTab = ref<'write' | 'preview'>('write')
const keywordInput = ref('')
const extraError = ref('')
const saving = ref(false)
const deleting = ref(false)
const pendingDelete = ref(false)
const saveError = ref('')
const showHistory = ref(false)
const stale = ref(false)
const showLatestDiff = ref(false)
const staleLatest = ref<LibraryCardDetail | null>(null)
const bodyEl = ref<HTMLTextAreaElement | null>(null)
const nameEl = ref<HTMLInputElement | null>(null)

let loadSeq = 0

function snapshot(): string {
  return JSON.stringify({ deck: deckId.value, folder: folder.value, name: name.value, form })
}

const dirty = computed(() => snapshot() !== pristine.value)
const canSave = computed(
  () =>
    mode.value === 'edit' &&
    dirty.value &&
    !!deckId.value &&
    !!name.value.trim() &&
    !nameError.value &&
    !extraError.value &&
    !saving.value,
)
const isLink = computed(() => !!detail.value && (!!detail.value.card.link || detail.value.card.dangling))
const deckOptions = computed<SelectOption[]>(() =>
  state.library.decks.map((deck) => ({ value: deck.id, label: deck.name })),
)
const nameError = computed(() => {
  const leaf = name.value.trim()
  return leaf && !ENTRY_NAME.test(leaf) ? t('library.nameRule') : ''
})
/** The deck's folders (with the empty ones made in the tree), and the
 *  card's own even when the deck has no such folder yet. */
const folderOptions = computed<SelectOption[]>(() => {
  const paths = new Set<string>()
  const addAll = (dir: string): void => {
    const parts = dir.split('/')
    for (let i = 1; i <= parts.length; i++) paths.add(parts.slice(0, i).join('/'))
  }
  for (const card of state.library.cards) {
    if (card.deck === deckId.value && card.path.includes('/')) {
      addAll(card.path.slice(0, card.path.lastIndexOf('/')))
    }
  }
  for (const key of state.library.folders) {
    if (key.startsWith(`${deckId.value}:`)) addAll(key.slice(deckId.value.length + 1))
  }
  if (folder.value) addAll(folder.value)
  return [
    { value: '', label: t('library.root') },
    ...[...paths].sort().map((dir) => ({ value: dir, label: dir, icon: 'mount' })),
  ]
})
const deckName = computed(() => state.library.decks.find((deck) => deck.id === deckId.value)?.name ?? '')
const title = computed(() => path.value.split('/').at(-1) || path.value || t('library.newCard'))
const dir = computed(() => (path.value.includes('/') ? path.value.slice(0, path.value.lastIndexOf('/')) : ''))
/** What the reader shows: a link shows its target, never its own file. */
const shown = computed(() => {
  if (detail.value?.target_view) return formOfView(detail.value.target_view)
  return detail.value ? form : blankForm()
})
const readView = computed<CardView>(
  () =>
    props.override ?? {
      description: shown.value.description,
      keywords: shown.value.keywords,
      meta: {},
      body: shown.value.body,
    },
)
const target = computed(() => detail.value?.card.link ?? null)
/** Relative links in the shown body resolve beside it: a link shows its target's. */
const linkBase = computed(() => target.value ?? { deck: props.deck, path: props.path })
const renderedBody = computed(() =>
  renderMarkdown(readView.value.body, { breaks: false, base: linkBase.value }),
)
const targetDeckName = computed(() =>
  target.value
    ? (state.library.decks.find((deck) => deck.id === target.value!.deck)?.name ?? target.value.deck)
    : '',
)
const extraEntries = computed<[string, unknown][]>(() => {
  if (props.override) {
    return Object.entries(props.override.meta).filter(
      ([key]) => !['description', 'keywords', 'scope'].includes(key),
    )
  }
  const text = shown.value.extra.trim()
  if (!text) return []
  try {
    const parsed: unknown = parseYaml(text)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      return Object.entries(parsed as Record<string, unknown>)
    }
  } catch {
    // The reader shows what it can; the form reports the parse error.
  }
  return []
})

function formatValue(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2)
}

function applyDetail(next: LibraryCardDetail): void {
  detail.value = next
  deckId.value = next.card.deck
  path.value = next.card.path
  base.value = next.view_rev
  Object.assign(form, formOfView(next.view))
  pristine.value = snapshot()
  mode.value = 'read'
  pendingDelete.value = false
  saveError.value = ''
  stale.value = false
  showLatestDiff.value = false
  staleLatest.value = null
}

async function load(deck: string, cardPath: string): Promise<void> {
  const seq = ++loadSeq
  loading.value = true
  try {
    const next = await loadCard(deck, cardPath)
    if (seq === loadSeq) applyDetail(next)
  } catch (err) {
    if (seq === loadSeq) {
      // Never leave a previous card under the newly selected row.
      detail.value = null
      saveError.value = (err as Error).message
    }
  } finally {
    if (seq === loadSeq) loading.value = false
  }
}

function startNew(deck: string, prefix = ''): void {
  const target = deck || state.library.decks[0]?.id || ''
  detail.value = null
  loading.value = false
  deckId.value = target
  path.value = prefix
  base.value = ''
  Object.assign(form, blankForm())
  pristine.value = snapshot()
  mode.value = 'edit'
  saveError.value = ''
  stale.value = false
  pendingDelete.value = false
  void nextTick(() => nameEl.value?.focus())
}

watch(
  () => [props.deck, props.path, props.isNew] as const,
  ([deck, cardPath, isNew]) => {
    showHistory.value = false
    saveError.value = ''
    if (isNew) {
      startNew(deck, cardPath)
      return
    }
    // Saving a card in place needs no reload.
    if (detail.value && detail.value.card.deck === deck && detail.value.card.path === cardPath) {
      return
    }
    // Keep the current card on screen while the next one loads; the reader
    // cross-fades when it arrives instead of blinking through the spinner.
    void load(deck, cardPath)
  },
  { immediate: true },
)

// ---- read / edit ----

function edit(): void {
  if (!detail.value) return
  // The drawer may open before the library has ever loaded its decks.
  if (!deckOptions.value.length) void refreshLibrary()
  Object.assign(form, formOfView(detail.value.view))
  deckId.value = detail.value.card.deck
  path.value = detail.value.card.path
  base.value = detail.value.view_rev
  pristine.value = snapshot()
  mode.value = 'edit'
  bodyTab.value = 'write'
}

/** True when leaving may discard work: the host asks before closing. */
function confirmLeave(): boolean {
  if (mode.value !== 'edit') return true
  return !dirty.value || window.confirm(t('common.discardPrompt'))
}

defineExpose({ confirmLeave })

function cancel(): void {
  if (!confirmLeave()) return
  if (!detail.value) {
    emit('close')
    return
  }
  applyDetail(detail.value)
}

/** A link card names its target: open it in a new window over this one. */
function openTarget(): void {
  const link = target.value
  if (!link) return
  openCardWindow(link.deck, link.path)
}

function openHistory(): void {
  pendingDelete.value = false
  showHistory.value = true
}

/** Open the live card's file in the library (from the drawer). */
function openInLibrary(closeMenu: () => void): void {
  closeMenu()
  const card = detail.value?.card
  if (!card) return
  emit('close')
  openLibrary(card.deck, card.path)
}

// ---- the form ----

function addKeyword(): void {
  for (const part of keywordInput.value.split(/[,，]/)) {
    const keyword = part.trim()
    if (keyword && !form.keywords.includes(keyword)) form.keywords.push(keyword)
  }
  keywordInput.value = ''
}

function onKeywordKey(e: KeyboardEvent): void {
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    addKeyword()
  } else if (e.key === 'Backspace' && !keywordInput.value && form.keywords.length) {
    form.keywords.pop()
  }
}

function removeKeyword(index: number): void {
  form.keywords.splice(index, 1)
}

function autoGrow(): void {
  const el = bodyEl.value
  if (!el) return
  el.style.height = 'auto'
  el.style.height = `${el.scrollHeight}px`
}

watch(bodyEl, (el) => {
  if (el) void nextTick(autoGrow)
})
watch(
  () => form.body,
  () => void nextTick(autoGrow),
)
watch(
  () => form.extra,
  (text) => {
    if (!text.trim()) {
      extraError.value = ''
      return
    }
    try {
      const parsed: unknown = parseYaml(text)
      extraError.value =
        parsed && typeof parsed === 'object' && !Array.isArray(parsed) ? '' : t('library.yamlError')
    } catch {
      extraError.value = t('library.yamlError')
    }
  },
)

// ---- save / delete ----

function draftView(): string {
  try {
    return viewOfForm(form)
  } catch {
    return ''
  }
}

async function loadLatest(): Promise<void> {
  const at = detail.value
    ? { deck: detail.value.card.deck, path: detail.value.card.path }
    : { deck: deckId.value, path: path.value }
  try {
    staleLatest.value = await loadCard(at.deck, at.path)
  } catch {
    staleLatest.value = null
  }
}

async function save(): Promise<void> {
  if (!canSave.value) return
  saving.value = true
  saveError.value = ''
  let view: string
  try {
    view = viewOfForm(form)
  } catch (err) {
    saveError.value = `${t('library.yamlError')}: ${(err as Error).message}`
    saving.value = false
    return
  }
  const nextPath = path.value.trim()
  const relocated =
    !detail.value || deckId.value !== detail.value.card.deck || nextPath !== detail.value.card.path
  try {
    const saved = await saveCard({
      deck: deckId.value,
      path: nextPath,
      view,
      base: base.value,
      ...(detail.value && relocated
        ? { from: { deck: detail.value.card.deck, path: detail.value.card.path } }
        : {}),
    })
    applyDetail(saved)
    toast('info', t('library.saved'))
    if (relocated) emit('moved', saved.card.deck, saved.card.path)
  } catch (err) {
    if (err instanceof RpcError && err.code === -32010) {
      stale.value = true
      await loadLatest()
    } else {
      saveError.value = `${t('library.saveError')}: ${(err as Error).message}`
    }
  } finally {
    saving.value = false
  }
}

function keepMine(): void {
  if (!staleLatest.value) return
  base.value = staleLatest.value.view_rev
  stale.value = false
  showLatestDiff.value = false
  void save()
}

function discardMine(): void {
  const latest = staleLatest.value
  stale.value = false
  showLatestDiff.value = false
  if (latest) applyDetail(latest)
}

async function remove(close: () => void): Promise<void> {
  if (!detail.value) return
  deleting.value = true
  saveError.value = ''
  try {
    await deleteCard(detail.value.card.deck, detail.value.card.path, detail.value.view_rev)
    toast('info', t('library.deleted'))
    pendingDelete.value = false
    close()
    emit('close')
  } catch (err) {
    saveError.value = `${t('library.deleteError')}: ${(err as Error).message}`
    pendingDelete.value = false
    close()
  } finally {
    deleting.value = false
  }
}

// ---- keyboard ----

function onKey(e: KeyboardEvent): void {
  if (e.key === 'Escape' && showHistory.value) {
    e.preventDefault()
    e.stopPropagation()
    showHistory.value = false
  } else if (e.key === 'Escape' && mode.value === 'edit') {
    e.preventDefault()
    e.stopPropagation()
    cancel()
  } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's' && mode.value === 'edit') {
    e.preventDefault()
    e.stopPropagation()
    void save()
  }
}

const draftDiff = computed(() =>
  stale.value && staleLatest.value ? diffLines(staleLatest.value.view, draftView()) : [],
)
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col" @keydown="onKey">
    <div
      v-if="!showHistory"
      class="mt-2 flex h-8 flex-none items-center gap-1"
      :class="mode === 'read' ? 'mx-auto w-full max-w-[var(--col-max)] px-6 sm:px-8' : 'px-4 sm:px-8'"
    >
      <h2 v-if="mode === 'edit'" class="min-w-0 flex-1 truncate text-title font-bold">
        {{ detail ? title : t('library.newCard') }}
      </h2>
      <span v-else class="min-w-0 flex-1" />

      <template v-if="mode === 'read' && detail">
        <button
          v-if="!isLink && !override"
          type="button"
          class="icon-btn size-8"
          :title="t('library.edit')"
          :aria-label="t('library.edit')"
          @click="edit"
        >
          <Icon name="edit" :size="16" />
        </button>
        <button
          type="button"
          class="icon-btn size-8"
          :title="t('library.history')"
          :aria-label="t('library.history')"
          @click="openHistory"
        >
          <Icon name="history" :size="16" />
        </button>
        <Popover menu role="menu" :label="t('library.more')" :to="overlay">
          <template #trigger="{ open, toggle: toggleMenu }">
            <button
              type="button"
              class="icon-btn size-8"
              :title="t('library.more')"
              :aria-label="t('library.more')"
              aria-haspopup="menu"
              :aria-expanded="open"
              @click="toggleMenu()"
            >
              <Icon name="more" :size="16" />
            </button>
          </template>
          <template #default="{ close }">
            <button
              v-if="!inLibrary"
              type="button"
              role="menuitem"
              :class="MENU_ITEM"
              @click="openInLibrary(close)"
            >
              <Icon name="external" :size="14" />
              {{ t('library.openInLibrary') }}
            </button>
            <div v-if="!inLibrary" class="mx-1 my-1 h-px bg-line-soft/70" />
            <button
              v-if="!pendingDelete"
              type="button"
              role="menuitem"
              :class="MENU_DANGER"
              @click="pendingDelete = true"
            >
              <Icon name="trash" :size="14" />
              {{ t('library.delete') }}
            </button>
            <div v-else class="flex max-w-64 flex-col gap-1.5 px-1.5 py-1">
              <p class="text-secondary text-ink">{{ t('library.deleteCardConfirm', { name: title }) }}</p>
              <p v-if="detail.linked" class="text-micro leading-relaxed text-danger">
                {{ t('library.linkedWarning') }}
              </p>
              <div class="mt-1 flex justify-end gap-1.5">
                <button type="button" class="btn" @click="pendingDelete = false">
                  {{ t('common.cancel') }}
                </button>
                <button type="button" class="btn btn-danger" :disabled="deleting" @click="remove(close)">
                  {{ t('library.delete') }}
                </button>
              </div>
            </div>
          </template>
        </Popover>
      </template>
      <slot name="trailing" />
    </div>

    <p
      v-if="saveError && mode === 'read'"
      class="mx-4 mb-2 flex-none rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-meta text-danger sm:mx-8"
    >
      {{ saveError }}
    </p>

    <CardHistory
      v-if="showHistory && detail"
      :deck="detail.card.deck"
      :path="detail.card.path"
      @back="showHistory = false"
    >
      <template #actions>
        <slot name="trailing" />
      </template>
    </CardHistory>

    <template v-else-if="mode === 'read' && detail">
      <!-- One grid cell stacks the cross-fading cards; unlike absolute
           positioning it keeps the content height, so the drawer (only a
           max-height) sizes to the card. -->
      <div class="grid min-h-0 flex-1 grid-rows-[minmax(0,1fr)]">
        <Transition
          enter-active-class="transition duration-200 ease-soft"
          enter-from-class="translate-y-1 opacity-0"
          leave-active-class="pointer-events-none transition-opacity duration-150 ease-soft"
          leave-to-class="opacity-0"
        >
          <article
            :key="`${detail.card.deck}:${detail.card.path}`"
            class="scroll-gutter col-start-1 row-start-1 min-h-0 overflow-y-auto"
          >
            <CardFace
              class="mx-auto w-full max-w-[var(--col-max)] px-6 pt-2 pb-6 sm:px-8 sm:pb-7"
              :dir="dir"
              :title="title"
              :description="readView.description"
              :keywords="readView.keywords"
              :html="renderedBody"
            >
              <template #meta>
                <slot name="meta">
                  <p v-if="shown.pinned" class="mt-3 flex items-center gap-1.5 text-meta text-memory">
                    <Icon name="pin" :size="14" />
                    {{ t('library.pinned') }}
                  </p>
                </slot>
              </template>
              <template #subtitle>
                <button
                  v-if="target"
                  type="button"
                  class="mt-2 flex max-w-full items-center gap-1.5 text-meta text-ink-3 hover:text-brand-strong"
                  @click="openTarget"
                >
                  <Icon name="link" :size="12" class="flex-none" />
                  <span class="min-w-0 truncate">
                    {{ t('library.linkTo', { deck: targetDeckName, path: target.path }) }}
                  </span>
                </button>
                <p
                  v-else-if="detail.card.dangling"
                  class="mt-2 flex items-center gap-1.5 text-meta text-danger"
                >
                  <Icon name="link" :size="12" class="flex-none" />
                  {{ t('library.dangling') }}
                </p>
              </template>
              <template #notice>
                <slot name="notice" />
              </template>
              <template #details>
                <slot name="details">
                  <dt class="text-ink-3">{{ t('library.deck') }}</dt>
                  <dd>{{ deckName }}</dd>
                  <dt class="text-ink-3">{{ t('cards.drawer.path') }}</dt>
                  <dd>{{ detail.card.path }}</dd>
                  <template v-for="[key, value] in extraEntries" :key="key">
                    <dt class="text-ink-3">{{ key }}</dt>
                    <dd class="whitespace-pre-wrap">{{ formatValue(value) }}</dd>
                  </template>
                </slot>
              </template>
            </CardFace>
          </article>
        </Transition>
      </div>
    </template>

    <form v-else-if="mode === 'edit'" class="flex min-h-0 flex-1 flex-col" @submit.prevent="save">
      <div class="scroll-gutter min-h-0 flex-1 overflow-y-auto px-4 pt-1 pb-4 sm:px-8">
        <div
          v-if="stale"
          class="mb-4 rounded-control border border-brand/30 bg-brand-soft/60 px-3 py-2.5 text-secondary"
        >
          <p class="font-semibold text-ink">{{ t('library.stale') }}</p>
          <div class="mt-2 flex flex-wrap items-center gap-2">
            <button type="button" class="btn" @click="showLatestDiff = !showLatestDiff">
              {{ t('library.showLatest') }}
            </button>
            <button type="button" class="btn btn-primary" :disabled="!staleLatest" @click="keepMine">
              {{ t('library.keepMine') }}
            </button>
            <button type="button" class="btn" @click="discardMine">
              {{ t('library.discardMine') }}
            </button>
          </div>
          <div
            v-if="showLatestDiff"
            class="mt-2 max-h-56 overflow-auto rounded-control bg-surface p-2 font-mono text-micro"
          >
            <p class="mb-1 font-sans font-semibold text-ink-3">{{ t('library.draft') }}</p>
            <div
              v-for="(line, i) in draftDiff"
              :key="i"
              class="whitespace-pre-wrap"
              :class="line.sign === '+' ? 'text-pine' : line.sign === '-' ? 'text-danger' : 'text-ink-3'"
            >
              {{ line.sign }}{{ line.text }}
            </div>
          </div>
        </div>

        <div class="flex flex-col gap-4">
          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('library.name') }}</span>
            <input
              ref="nameEl"
              v-model="name"
              type="text"
              class="w-full"
              :aria-invalid="!!nameError"
              autocomplete="off"
              spellcheck="false"
            />
            <span v-if="nameError" class="text-micro text-danger">{{ nameError }}</span>
          </label>

          <div class="grid gap-4 sm:grid-cols-2">
            <label class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('library.deck') }}</span>
              <Select v-model="deckId" :options="deckOptions" :label="t('library.deck')" :to="overlay" />
            </label>
            <label class="flex flex-col gap-1.5">
              <span class="field-label">{{ t('library.folder') }}</span>
              <Select v-model="folder" :options="folderOptions" :label="t('library.folder')" :to="overlay" />
            </label>
          </div>

          <label class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('library.description') }}</span>
            <input v-model="form.description" type="text" class="w-full" autocomplete="off" />
          </label>

          <div class="flex flex-col gap-1.5">
            <span class="field-label">{{ t('library.keywords') }}</span>
            <div class="flex flex-wrap items-center gap-1.5">
              <span
                v-for="(keyword, i) in form.keywords"
                :key="i"
                class="keyword-tag flex items-center gap-1"
              >
                {{ keyword }}
                <button
                  type="button"
                  class="text-ink-3 hover:text-danger"
                  :aria-label="t('common.cancel')"
                  @click="removeKeyword(i)"
                >
                  <Icon name="close" :size="12" />
                </button>
              </span>
              <input
                v-model="keywordInput"
                type="text"
                class="w-40"
                :placeholder="t('library.keywordsHint')"
                autocomplete="off"
                @keydown="onKeywordKey"
                @blur="addKeyword"
              />
            </div>
          </div>

          <label
            class="flex w-fit cursor-pointer items-center gap-2 text-secondary text-ink"
            :title="t('library.pinnedHint')"
          >
            <input v-model="form.pinned" type="checkbox" />
            {{ t('library.pinned') }}
          </label>

          <div class="flex flex-col gap-1.5">
            <div class="flex items-center gap-2">
              <span class="field-label">{{ t('library.body') }}</span>
              <div class="ml-auto flex items-center gap-1 text-micro font-semibold">
                <button
                  v-for="tab in ['write', 'preview'] as const"
                  :key="tab"
                  type="button"
                  class="rounded-control px-2 py-1"
                  :class="bodyTab === tab ? 'bg-surface-2 text-ink' : 'text-ink-3 hover:text-ink'"
                  :aria-pressed="bodyTab === tab"
                  @click="bodyTab = tab"
                >
                  {{ t(tab === 'write' ? 'library.write' : 'library.preview') }}
                </button>
              </div>
            </div>
            <textarea
              v-if="bodyTab === 'write'"
              ref="bodyEl"
              v-model="form.body"
              rows="12"
              class="w-full resize-none font-card text-message"
              spellcheck="false"
            />
            <div
              v-else
              class="md min-h-[12rem] rounded-control border border-line-soft bg-surface px-3 py-2 text-ink"
              v-html="renderMarkdown(form.body, { breaks: false, base: linkBase })"
            />
          </div>

          <details class="flex flex-col gap-1.5 text-meta text-ink-2">
            <summary class="w-fit cursor-pointer font-semibold">{{ t('library.advanced') }}</summary>
            <textarea
              v-model="form.extra"
              rows="4"
              class="mt-1 w-full font-mono text-meta"
              placeholder="key: value"
              :aria-label="t('library.advanced')"
              spellcheck="false"
            />
            <span v-if="extraError" class="text-danger">{{ extraError }}</span>
          </details>
        </div>
      </div>

      <footer class="flex flex-none flex-col gap-2 border-t border-line-soft px-4 py-3 sm:px-8">
        <p v-if="saveError" class="text-meta text-danger">{{ saveError }}</p>
        <div class="flex items-center gap-2">
          <span v-if="dirty" class="text-meta text-ink-3">{{ t('library.unsaved') }}</span>
          <button type="button" class="btn ml-auto" @click="cancel">{{ t('library.cancel') }}</button>
          <button type="submit" class="btn btn-primary" :disabled="!canSave">
            {{ t('library.save') }}
          </button>
        </div>
      </footer>
    </form>

    <div v-else-if="loading" class="flex flex-1 items-center justify-center text-secondary text-ink-3">…</div>
    <div v-else class="flex flex-1 flex-col items-center justify-center gap-3 px-6 text-center">
      <p class="text-secondary text-ink-3">{{ saveError || t('library.pick') }}</p>
    </div>
  </div>
</template>
