<script setup lang="ts">
// The library tree: every deck is a root, folders and cards nest below it.
// Rendered as a flattened list so the ARIA tree keyboard model stays simple.
// Every row has the same action menu; names are edited in place, and cards
// and folders move by dragging them onto a deck or folder.
import { computed, nextTick, ref, watch } from 'vue'
import { t } from '../../i18n'
import { buildLibraryTree, type TreeNode } from '../../lib/libraryTree'
import { openAssistantSettings } from '../../lib/layout'
import {
  addFolder,
  closeLibrary,
  createDeck,
  deleteCard,
  deleteDeck,
  deleteFolder,
  loadCard,
  moveEntry,
  newCard,
  openCard,
  renameDeck,
  saveCard,
  state,
} from '../../lib/store'
import { toast } from '../../lib/state/ui'
import type { LibraryCard } from '../../lib/types'
import Icon from '../ui/Icon.vue'
import Popover from '../ui/Popover.vue'

const DECK_NAME = /^[a-z][a-z0-9-]*$/
const ENTRY_NAME = /^[A-Za-z0-9_-][A-Za-z0-9._-]*$/

const MENU_ITEM =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-ink-2 transition-colors duration-150 hover:bg-surface-2 hover:text-ink'
const MENU_DANGER =
  'flex w-full items-center gap-2 rounded-control px-2.5 py-1.5 text-left text-secondary text-danger transition-colors duration-150 hover:bg-danger-soft'

const query = computed(() => state.library.query.trim())
const searching = computed(() => query.value.length > 0)

const tree = computed(() => {
  if (!searching.value) {
    const folders = [...state.library.folders].map((key) => {
      const at = key.indexOf(':')
      return { deck: key.slice(0, at), path: key.slice(at + 1) }
    })
    return buildLibraryTree(state.library.decks, state.library.cards, folders)
  }
  const hits = state.library.hits ?? []
  const decks = state.library.decks.filter((deck) => hits.some((card) => card.deck === deck.id))
  return buildLibraryTree(decks, hits)
})

interface Visible {
  node: TreeNode
  depth: number
}

type Branch = Extract<TreeNode, { children: TreeNode[] }>
type FolderNode = Extract<TreeNode, { kind: 'folder' }>

const isBranch = (node: TreeNode): node is Branch => node.kind !== 'card'
const isExpanded = (node: TreeNode): boolean => searching.value || state.library.expanded.has(node.key)

const visible = computed<Visible[]>(() => {
  const out: Visible[] = []
  const walk = (nodes: TreeNode[], depth: number): void => {
    for (const node of nodes) {
      out.push({ node, depth })
      if (isBranch(node) && isExpanded(node)) walk(node.children, depth + 1)
    }
  }
  walk(tree.value, 0)
  return out
})

const treeEl = ref<HTMLElement | null>(null)
const focused = ref(0)

function isSelected(node: TreeNode): boolean {
  if (node.kind !== 'card') return false
  const open = state.library.open
  return open !== null && !open.isNew && open.deck === node.card.deck && open.path === node.card.path
}

const cardName = (card: LibraryCard): string => card.path.split('/').at(-1) || card.path

function label(node: TreeNode): string {
  if (node.kind === 'deck') return node.deck.name
  if (node.kind === 'folder') return node.name
  return cardName(node.card)
}

/** The deck id and path of a row (`""` for a deck root). */
function place(node: TreeNode): { deck: string; path: string } {
  if (node.kind === 'deck') return { deck: node.deck.id, path: '' }
  if (node.kind === 'folder') return { deck: node.deck, path: node.path }
  return { deck: node.card.deck, path: node.card.path }
}

const parentOf = (path: string): string => (path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '')
const join = (dir: string, name: string): string => (dir ? `${dir}/${name}` : name)

/** The cards a deck or folder row holds, at any depth. */
function cardCount(node: TreeNode): number {
  if (node.kind === 'deck') return node.deck.cards
  if (node.kind === 'card') return 1
  return state.library.cards.filter(
    (card) => card.deck === node.deck && card.path.startsWith(`${node.path}/`),
  ).length
}

function toggle(node: TreeNode): void {
  if (searching.value || !isBranch(node)) return
  const set = state.library.expanded
  if (set.has(node.key)) set.delete(node.key)
  else set.add(node.key)
}

function activate(node: TreeNode): void {
  if (node.kind === 'card') openCard(node.card.deck, node.card.path)
  else toggle(node)
}

function focusRow(index: number): void {
  if (index < 0 || index >= visible.value.length) return
  focused.value = index
  void nextTick(() => {
    treeEl.value?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.focus()
  })
}

function onRowClick(index: number, node: TreeNode): void {
  focused.value = index
  if (draft.value?.kind === 'rename' && draft.value.key === node.key) return
  activate(node)
}

function onKey(e: KeyboardEvent): void {
  // Form controls (the name inputs, the row menus) keep their own keys.
  if ((e.target as HTMLElement).closest('input, textarea, button')) return
  const index = focused.value
  const item = visible.value[index]
  if (!item) return
  const { node, depth } = item
  switch (e.key) {
    case 'ArrowDown':
      e.preventDefault()
      focusRow(index + 1)
      break
    case 'ArrowUp':
      e.preventDefault()
      focusRow(index - 1)
      break
    case 'Home':
      e.preventDefault()
      focusRow(0)
      break
    case 'End':
      e.preventDefault()
      focusRow(visible.value.length - 1)
      break
    case 'ArrowRight': {
      e.preventDefault()
      if (!isBranch(node)) break
      if (!isExpanded(node)) {
        toggle(node)
      } else if (visible.value[index + 1]?.depth === depth + 1) {
        focusRow(index + 1)
      }
      break
    }
    case 'ArrowLeft': {
      e.preventDefault()
      if (isBranch(node) && isExpanded(node) && !searching.value) {
        toggle(node)
        break
      }
      for (let i = index - 1; i >= 0; i--) {
        if (visible.value[i]!.depth < depth) {
          focusRow(i)
          break
        }
      }
      break
    }
    case 'Enter':
    case ' ':
      e.preventDefault()
      activate(node)
      break
    case 'F2':
      e.preventDefault()
      beginRename(node)
      break
  }
}

// Opening a card (from the tree or the drawer) expands its ancestors; the
// later watcher scrolls the now-visible row into place.
watch(
  () => state.library.open,
  async (open) => {
    if (!open) return
    state.library.expanded.add(`deck:${open.deck}`)
    const parts = open.path.split('/')
    parts.pop()
    let path = ''
    for (const part of parts) {
      if (!part) continue
      path = path ? `${path}/${part}` : part
      state.library.expanded.add(`dir:${open.deck}:${path}`)
    }
    if (open.isNew) return
    await nextTick()
    const index = visible.value.findIndex(
      (entry) =>
        entry.node.kind === 'card' &&
        entry.node.card.deck === open.deck &&
        entry.node.card.path === open.path,
    )
    if (index < 0) return
    focused.value = index
    await nextTick()
    treeEl.value?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: 'nearest' })
  },
  { immediate: true },
)

// Expansion changes the row list; keep the roving focus in range without
// stealing it from the row the user is on.
watch(visible, (list) => {
  if (!list.length) focused.value = -1
  else if (focused.value < 0 || focused.value >= list.length) focused.value = 0
})

// A jump from elsewhere lands on a deck row. The request may arrive before
// the decks do, so it survives until the tree can place it.
watch(
  [() => state.library.focusDeck, tree],
  async ([deck]) => {
    if (!deck) return
    state.library.expanded.add(`deck:${deck}`)
    const index = visible.value.findIndex(
      (entry) => entry.node.kind === 'deck' && entry.node.deck.id === deck,
    )
    if (index < 0) return
    state.library.focusDeck = null
    focused.value = index
    await nextTick()
    treeEl.value?.querySelector<HTMLElement>(`[data-index="${index}"]`)?.scrollIntoView({ block: 'nearest' })
  },
  { immediate: true },
)

// ---- names edited in place: rename, new folder, new deck ----

type Draft =
  | { kind: 'rename'; key: string; node: TreeNode }
  | { kind: 'folder'; parent: string; deck: string; path: string }
  | { kind: 'deck' }

const draft = ref<Draft | null>(null)
const draftName = ref('')
const draftError = ref('')
const draftBusy = ref(false)
const draftInput = ref<HTMLInputElement | null>(null)
// A function ref: the inputs sit inside the row `v-for`.
const setDraftInput = (el: unknown): void => {
  draftInput.value = (el as HTMLInputElement | null) ?? null
}

watch(draftName, () => {
  draftError.value = ''
})

function startDraft(next: Draft, name = ''): void {
  draft.value = next
  draftName.value = name
  draftError.value = ''
  void nextTick(() => {
    draftInput.value?.focus()
    draftInput.value?.select()
  })
}

function beginRename(node: TreeNode): void {
  if (searching.value) return
  startDraft({ kind: 'rename', key: node.key, node }, label(node))
}

function beginNewFolder(node: TreeNode): void {
  const { deck, path } = place(node)
  state.library.expanded.add(node.key)
  startDraft({ kind: 'folder', parent: node.key, deck, path })
}

function beginNewDeck(): void {
  startDraft({ kind: 'deck' })
}

function cancelDraft(): void {
  draft.value = null
  draftError.value = ''
}

/** True when a card or folder named `name` already sits in `dir`. */
function taken(deck: string, dir: string, name: string): boolean {
  const path = join(dir, name)
  return (
    state.library.cards.some(
      (card) => card.deck === deck && (card.path === path || card.path.startsWith(`${path}/`)),
    ) || state.library.folders.has(`${deck}:${path}`)
  )
}

async function commitDraft(): Promise<void> {
  const current = draft.value
  if (!current || draftBusy.value) return
  const name = draftName.value.trim()
  const isDeck = current.kind === 'deck' || (current.kind === 'rename' && current.node.kind === 'deck')
  if (!name || (current.kind === 'rename' && name === label(current.node))) return cancelDraft()
  if (!(isDeck ? DECK_NAME : ENTRY_NAME).test(name)) {
    draftError.value = t(isDeck ? 'assistants.nameHint' : 'library.nameRule')
    return
  }
  draftBusy.value = true
  draftError.value = ''
  try {
    if (current.kind === 'deck') {
      await createDeck(name)
    } else if (current.kind === 'folder') {
      if (taken(current.deck, current.path, name)) throw new Error(t('library.nameTaken'))
      addFolder(current.deck, join(current.path, name))
    } else {
      const node = current.node
      if (node.kind === 'deck') {
        await renameDeck(node.deck.id, name)
      } else {
        const { deck, path } = place(node)
        if (taken(deck, parentOf(path), name)) throw new Error(t('library.nameTaken'))
        await moveEntry(deck, path, join(parentOf(path), name), node.kind === 'folder')
      }
    }
    if (draft.value === current) draft.value = null
  } catch (err) {
    draftError.value = (err as Error).message
    void nextTick(() => draftInput.value?.focus())
  } finally {
    draftBusy.value = false
  }
}

/** Leaving the field keeps a valid name and drops an empty or refused one. */
function onDraftBlur(): void {
  if (draftBusy.value) return
  if (!draftName.value.trim() || draftError.value) cancelDraft()
  else void commitDraft()
}

function onDraftKey(e: KeyboardEvent): void {
  if (e.key === 'Enter') {
    e.preventDefault()
    void commitDraft()
  } else if (e.key === 'Escape') {
    e.preventDefault()
    e.stopPropagation()
    cancelDraft()
  }
}

const isRenaming = (node: TreeNode): boolean => draft.value?.kind === 'rename' && draft.value.key === node.key

// ---- delete (a confirm step inside the row menu) ----

const confirming = ref<string | null>(null)
/** For a card: whether other cards link to it, once loaded. */
const confirmLinked = ref(false)
const deleting = ref(false)

async function askDelete(node: TreeNode): Promise<void> {
  confirming.value = node.key
  confirmLinked.value = false
  if (node.kind !== 'card') return
  try {
    const detail = await loadCard(node.card.deck, node.card.path)
    if (confirming.value === node.key) confirmLinked.value = detail.linked
  } catch {
    // The delete itself reports a card that is gone.
  }
}

function deleteQuestion(node: TreeNode): string {
  const name = label(node)
  if (node.kind === 'card') return t('library.deleteCardConfirm', { name })
  const n = cardCount(node)
  if (node.kind === 'deck') return t('library.deleteDeckConfirm', { name, n })
  return n ? t('library.deleteFolderConfirm', { name, n }) : t('library.deleteCardConfirm', { name })
}

async function remove(node: TreeNode, close: () => void): Promise<void> {
  deleting.value = true
  try {
    if (node.kind === 'deck') {
      await deleteDeck(node.deck.id)
    } else if (node.kind === 'folder') {
      await deleteFolder(node.deck, node.path)
    } else {
      const detail = await loadCard(node.card.deck, node.card.path)
      await deleteCard(node.card.deck, node.card.path, detail.view_rev)
      const open = state.library.open
      if (open && !open.isNew && open.deck === node.card.deck && open.path === node.card.path) {
        state.library.open = null
      }
    }
    toast('info', t('library.deleted'))
    close()
  } catch (err) {
    toast('error', t('library.deleteError'), (err as Error).message)
  } finally {
    deleting.value = false
    confirming.value = null
  }
}

/** Run a menu item: close the menu first, so its focus handling is done. */
function pick(close: () => void, action: () => void): void {
  close()
  action()
}

function onMenuToggle(open: boolean, toggleMenu: () => void): void {
  if (!open) confirming.value = null
  toggleMenu()
}

function openAssistant(id: string): void {
  closeLibrary()
  openAssistantSettings(id)
}

// ---- drag to move ----

const dragged = ref<{ key: string; deck: string; path: string; folder: boolean } | null>(null)
const dropKey = ref<string | null>(null)

function canDrag(node: TreeNode): boolean {
  return node.kind !== 'deck' && !searching.value && !draft.value
}

function onDragStart(e: DragEvent, node: TreeNode): void {
  const { deck, path } = place(node)
  dragged.value = { key: node.key, deck, path, folder: node.kind === 'folder' }
  if (e.dataTransfer) {
    e.dataTransfer.effectAllowed = 'move'
    e.dataTransfer.setData('text/plain', path)
  }
}

function onDragEnd(): void {
  dragged.value = null
  dropKey.value = null
}

/** Whether the dragged row may land in `node` (a deck or folder). Folders
 *  stay in their deck; a card may change decks. */
function accepts(node: TreeNode): boolean {
  const from = dragged.value
  if (!from || !isBranch(node)) return false
  const to = place(node)
  if (to.deck === from.deck && to.path === parentOf(from.path)) return false
  if (!from.folder) return true
  return to.deck === from.deck && to.path !== from.path && !to.path.startsWith(`${from.path}/`)
}

function onDragOver(e: DragEvent, node: TreeNode): void {
  if (!accepts(node)) return
  e.preventDefault()
  if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'
  dropKey.value = node.key
}

function onDragLeave(node: TreeNode): void {
  if (dropKey.value === node.key) dropKey.value = null
}

async function onDrop(e: DragEvent, node: TreeNode): Promise<void> {
  const from = dragged.value
  onDragEnd()
  if (!from || !isBranch(node)) return
  e.preventDefault()
  const to = place(node)
  const name = from.path.split('/').at(-1)!
  if (taken(to.deck, to.path, name)) {
    toast('error', t('library.moveError'), t('library.nameTaken'))
    return
  }
  const dest = join(to.path, name)
  try {
    if (to.deck === from.deck) {
      await moveEntry(from.deck, from.path, dest, from.folder)
    } else {
      const detail = await loadCard(from.deck, from.path)
      await saveCard({
        deck: to.deck,
        path: dest,
        view: detail.view,
        base: detail.view_rev,
        from: { deck: from.deck, path: from.path },
      })
      const open = state.library.open
      if (open && !open.isNew && open.deck === from.deck && open.path === from.path) {
        state.library.open = { deck: to.deck, path: dest }
      }
    }
    state.library.expanded.add(node.key)
  } catch (err) {
    toast('error', t('library.moveError'), (err as Error).message)
  }
}

// A card's tooltip keeps the tree one line; search hits get a second.
function cardTitle(card: LibraryCard): string {
  return card.description || cardName(card)
}

const isFolder = (node: TreeNode): node is FolderNode => node.kind === 'folder'
</script>

<template>
  <div
    ref="treeEl"
    class="min-h-0 flex-1 overflow-y-auto px-2 py-2"
    role="tree"
    :aria-label="t('library.title')"
    @keydown="onKey"
  >
    <p
      v-if="searching && state.library.hits === null"
      class="px-2 py-12 text-center text-secondary text-ink-3"
    >
      …
    </p>
    <p v-else-if="searching && !visible.length" class="px-2 py-12 text-center text-secondary text-ink-3">
      {{ t('library.none', { q: query }) }}
    </p>
    <p
      v-else-if="!searching && !state.library.decks.length"
      class="px-2 py-8 text-center text-secondary text-ink-3"
    >
      {{ t('library.empty') }}
    </p>

    <template v-for="({ node, depth }, i) in visible" :key="node.key">
      <div
        :data-index="i"
        role="treeitem"
        :aria-level="depth + 1"
        :aria-expanded="isBranch(node) ? isExpanded(node) : undefined"
        :aria-selected="node.kind === 'card' ? isSelected(node) : undefined"
        :tabindex="i === focused ? 0 : -1"
        :title="
          node.kind === 'card'
            ? cardTitle(node.card)
            : node.kind === 'deck'
              ? (node.deck.description ?? undefined)
              : undefined
        "
        :draggable="canDrag(node)"
        class="group flex min-h-8 cursor-pointer items-center gap-1.5 rounded-control border pr-1 font-card outline-none transition-colors duration-150 hover:bg-surface-2/60 focus-visible:ring-2 focus-visible:ring-brand/25"
        :class="[
          node.kind === 'deck' ? 'font-semibold text-ink' : 'text-ink-2',
          node.kind === 'deck' && i > 0 ? 'mt-2' : '',
          dropKey === node.key
            ? 'border-memory/40 bg-memory-soft/70'
            : isSelected(node)
              ? 'border-memory/20 bg-memory-soft font-semibold text-ink hover:bg-memory-soft'
              : 'border-transparent',
          dragged?.key === node.key ? 'opacity-50' : '',
        ]"
        :style="{ paddingLeft: `${6 + depth * 14}px` }"
        @click="onRowClick(i, node)"
        @focus="focused = i"
        @dragstart="onDragStart($event, node)"
        @dragend="onDragEnd"
        @dragover="onDragOver($event, node)"
        @dragleave="onDragLeave(node)"
        @drop="onDrop($event, node)"
      >
        <Icon
          v-if="isBranch(node)"
          name="chevron"
          :size="12"
          class="flex-none text-ink-3 transition-transform duration-150"
          :class="isExpanded(node) ? 'rotate-90' : ''"
        />
        <span v-else class="w-3 flex-none" />
        <Icon
          :name="node.kind === 'deck' ? 'memory' : node.kind === 'folder' ? 'mount' : 'file'"
          :size="14"
          class="flex-none"
          :class="node.kind === 'deck' || isSelected(node) ? 'text-memory' : 'text-ink-3/70'"
        />

        <span v-if="isRenaming(node)" class="min-w-0 flex-1 py-0.5" @click.stop>
          <input
            :ref="setDraftInput"
            v-model="draftName"
            type="text"
            class="w-full px-1.5 py-0.5 font-sans text-secondary font-normal"
            :aria-label="t('library.rename')"
            :aria-invalid="!!draftError"
            autocomplete="off"
            spellcheck="false"
            :disabled="draftBusy"
            @keydown="onDraftKey"
            @blur="onDraftBlur"
          />
          <span v-if="draftError" class="block px-1 pt-0.5 font-sans text-micro font-normal text-danger">
            {{ draftError }}
          </span>
        </span>
        <span v-else class="min-w-0 flex-1 py-1">
          <span class="flex min-w-0 items-center gap-1">
            <span class="truncate" :class="node.kind === 'card' ? 'text-secondary' : ''">{{
              label(node)
            }}</span>
            <span
              v-if="node.kind === 'deck'"
              class="flex-none rounded-full bg-surface-2 px-1.5 py-px font-sans text-micro font-semibold text-ink-3 tabular-nums"
              >{{ node.deck.cards }}</span
            >
            <template v-if="node.kind === 'card'">
              <Icon
                v-if="node.card.pinned"
                name="pin"
                :size="14"
                class="text-memory"
                :aria-label="t('library.pinned')"
              />
              <Icon
                v-if="node.card.link || node.card.dangling"
                name="link"
                :size="14"
                class="flex-none"
                :class="node.card.dangling ? 'text-danger' : 'text-ink-3'"
                :aria-label="t(node.card.dangling ? 'library.dangling' : 'library.link')"
              />
            </template>
          </span>
          <span
            v-if="node.kind === 'card' && searching && node.card.snippet"
            class="line-clamp-1 font-sans text-micro text-ink-3"
          >
            {{ node.card.snippet }}
          </span>
        </span>

        <button
          v-if="node.kind === 'deck' && node.deck.assistant && !isRenaming(node)"
          type="button"
          class="flex flex-none items-center gap-1 rounded-full px-1.5 py-px font-sans text-micro font-medium text-agent transition-colors duration-150 hover:bg-agent-soft"
          :title="t('library.assistantLink')"
          @click.stop="openAssistant(node.deck.id)"
        >
          <Icon name="link" :size="12" />
          {{ t('library.assistant') }}
        </button>

        <Popover v-if="!isRenaming(node) && !searching" menu role="menu" :label="t('library.more')">
          <template #trigger="{ open, toggle: toggleMenu }">
            <button
              type="button"
              class="icon-btn size-7 flex-none opacity-100 lg:opacity-0 lg:group-hover:opacity-100 lg:focus-visible:opacity-100 lg:aria-expanded:opacity-100"
              :aria-label="t('library.more')"
              :title="t('library.more')"
              aria-haspopup="menu"
              :aria-expanded="open"
              @click.stop="onMenuToggle(open, toggleMenu)"
            >
              <Icon name="more" :size="16" />
            </button>
          </template>
          <template #default="{ close }">
            <div class="min-w-44 font-sans font-normal">
              <template v-if="confirming !== node.key">
                <template v-if="isBranch(node)">
                  <button
                    type="button"
                    role="menuitem"
                    :class="MENU_ITEM"
                    @click="
                      pick(close, () => newCard(place(node).deck, isFolder(node) ? `${node.path}/` : ''))
                    "
                  >
                    <Icon name="plus" :size="14" />
                    {{ t('library.newCard') }}
                  </button>
                  <button
                    type="button"
                    role="menuitem"
                    :class="MENU_ITEM"
                    @click="pick(close, () => beginNewFolder(node))"
                  >
                    <Icon name="mount" :size="14" />
                    {{ t('library.newFolder') }}
                  </button>
                </template>
                <button
                  type="button"
                  role="menuitem"
                  :class="MENU_ITEM"
                  @click="pick(close, () => beginRename(node))"
                >
                  <Icon name="edit" :size="14" />
                  {{ t('library.rename') }}
                </button>
                <template v-if="node.kind !== 'deck' || !node.deck.assistant">
                  <div class="mx-1 my-1 h-px bg-line-soft/70" />
                  <button type="button" role="menuitem" :class="MENU_DANGER" @click="askDelete(node)">
                    <Icon name="trash" :size="14" />
                    {{ t('library.delete') }}
                  </button>
                </template>
              </template>
              <div v-else class="flex max-w-64 flex-col gap-1.5 px-1.5 py-1">
                <p class="text-secondary text-ink">{{ deleteQuestion(node) }}</p>
                <p
                  v-if="node.kind === 'card' && confirmLinked"
                  class="text-micro leading-relaxed text-danger"
                >
                  {{ t('library.linkedWarning') }}
                </p>
                <p v-else-if="node.kind === 'deck'" class="text-micro leading-relaxed text-ink-3">
                  {{ t('library.deleteDeckNote') }}
                </p>
                <div class="mt-1 flex justify-end gap-1.5">
                  <button type="button" class="btn" @click="confirming = null">
                    {{ t('common.cancel') }}
                  </button>
                  <button
                    type="button"
                    class="btn btn-danger"
                    :disabled="deleting"
                    @click="remove(node, close)"
                  >
                    {{ t('library.delete') }}
                  </button>
                </div>
              </div>
            </div>
          </template>
        </Popover>
      </div>

      <div
        v-if="draft?.kind === 'folder' && draft.parent === node.key"
        class="flex min-h-8 items-start gap-1.5 py-0.5 pr-1"
        :style="{ paddingLeft: `${6 + (depth + 1) * 14}px` }"
      >
        <span class="w-3 flex-none" />
        <Icon name="mount" :size="14" class="mt-2 flex-none text-ink-3/70" />
        <span class="min-w-0 flex-1">
          <input
            :ref="setDraftInput"
            v-model="draftName"
            type="text"
            class="w-full px-1.5 py-0.5 text-secondary"
            :placeholder="t('library.folderName')"
            :aria-label="t('library.newFolder')"
            :aria-invalid="!!draftError"
            autocomplete="off"
            spellcheck="false"
            @keydown="onDraftKey"
            @blur="onDraftBlur"
          />
          <span v-if="draftError" class="block px-1 pt-0.5 text-micro text-danger">{{ draftError }}</span>
        </span>
      </div>
    </template>

    <div v-if="!searching" class="mt-1 border-t border-line-soft/60 pt-1">
      <div v-if="draft?.kind === 'deck'" class="flex items-start gap-1.5 px-1.5 py-1">
        <Icon name="memory" :size="14" class="mt-2 flex-none text-memory" />
        <span class="min-w-0 flex-1">
          <input
            :ref="setDraftInput"
            v-model="draftName"
            type="text"
            class="w-full px-1.5 py-0.5 text-secondary"
            :placeholder="t('library.newDeckName')"
            :aria-label="t('library.newDeckRow')"
            :aria-invalid="!!draftError"
            autocomplete="off"
            spellcheck="false"
            :disabled="draftBusy"
            @keydown="onDraftKey"
            @blur="onDraftBlur"
          />
          <span v-if="draftError" class="block px-1 pt-0.5 text-micro text-danger">{{ draftError }}</span>
        </span>
        <!-- mousedown.prevent keeps focus so the blur handler doesn't commit first. -->
        <button
          type="button"
          class="icon-btn mt-0.5 size-6 flex-none text-ink-3"
          :title="t('common.cancel')"
          :aria-label="t('common.cancel')"
          :disabled="draftBusy"
          @mousedown.prevent
          @click="cancelDraft"
        >
          <Icon name="close" :size="12" />
        </button>
      </div>
      <button
        v-else
        type="button"
        class="flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-meta text-ink-3 transition-colors duration-150 hover:bg-surface-2/60 hover:text-ink"
        @click="beginNewDeck"
      >
        <Icon name="plus" :size="14" />
        {{ t('library.newDeckRow') }}
      </button>
    </div>
  </div>
</template>
