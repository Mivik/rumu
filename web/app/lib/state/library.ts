// The card library page: every deck, independent of the mounted cards a
// session sees. Mutations refresh both this view and the chat's `state.cards`,
// so chips and the drawer follow. The open card is the page's item
// (`/decks/<deck>/<path>`), so Back walks the cards that were read.

import { computed } from 'vue'
import { t } from '../../i18n'
import { rumu } from '../api'
import { openPage, replacePage } from '../router'
import type { LibraryCard, LibraryCardDetail, LibraryCommit, LibraryDeck, LibraryVersion } from '../types'
import { refreshCards } from './sessions'
import { state } from './state'
import { toast } from './ui'

/** A card's place: its deck and path; `isNew` is a blank form at `path` (a
 *  folder prefix, or `""`). */
export interface OpenCard {
  deck: string
  path: string
  isNew?: boolean
}

/** The page item naming a card (a deck alone for its root). */
const cardItem = (deck: string, path: string): string => (path ? `${deck}/${path}` : deck)

/** The card open on the library page, if any. Deck ids hold no `/`. */
export const openedCard = computed<OpenCard | null>(() => {
  const p = state.page
  if (p?.page !== 'decks' || !p.item) return null
  const slash = p.item.indexOf('/')
  const deck = slash < 0 ? p.item : p.item.slice(0, slash)
  const path = slash < 0 ? '' : p.item.slice(slash + 1)
  if (!path && !p.fresh) return null
  return { deck, path, isNew: p.fresh }
})

/** Navigate to the library, at a card when given, and load it. */
export function openLibrary(deck: string | null = null, path: string | null = null): void {
  openPage('decks', deck && path ? cardItem(deck, path) : null)
  state.library.query = ''
  state.library.hits = null
  void refreshLibrary()
}

/** Open the library at a deck's row in the tree. */
export function openLibraryAtDeck(deck: string): void {
  openLibrary()
  state.library.focusDeck = deck
}

/** Open one card in the editor. */
export function openCard(deck: string, path: string): void {
  openPage('decks', cardItem(deck, path))
}

/** Open a blank card form in `deck`, prefilled with a folder prefix. */
export function newCard(deck: string | null = null, pathPrefix = ''): void {
  const target = deck || state.library.decks[0]?.id
  if (!target) return
  openPage('decks', cardItem(target, pathPrefix), true)
}

/** The open card is now at another place, or gone (`null`): correct the
 *  address in place. */
export function moveOpenCard(card: OpenCard | null): void {
  if (state.page?.page !== 'decks') return
  replacePage(card ? cardItem(card.deck, card.path) : null, !!card?.isNew)
}

/** Re-read the sidebar decks and every card row. */
export async function refreshLibrary(): Promise<void> {
  state.library.loading = true
  try {
    const [decks, cards] = await Promise.all([
      rumu.request<LibraryDeck[]>('library.decks'),
      rumu.request<LibraryCard[]>('library.cards'),
    ])
    state.library.decks = decks
    state.library.cards = cards
    // A mutation while the tree is filtered must refresh its hits too.
    if (state.library.query.trim()) void searchLibrary(state.library.query)
  } catch (err) {
    toast('error', t('library.loadError'), (err as Error).message)
  } finally {
    state.library.loading = false
  }
}

/** The query whose hits are current; an older response is dropped. */
let searchSeq = 0

/** Search every deck (the component debounces the input). */
export async function searchLibrary(query: string): Promise<void> {
  const seq = ++searchSeq
  state.library.query = query
  const q = query.trim()
  if (!q) {
    state.library.hits = null
    return
  }
  try {
    const hits = await rumu.request<LibraryCard[]>('library.search', { query: q })
    if (seq === searchSeq) state.library.hits = hits
  } catch (err) {
    if (seq === searchSeq) toast('error', t('library.loadError'), (err as Error).message)
  }
}

export function loadCard(deck: string, path: string): Promise<LibraryCardDetail> {
  return rumu.request<LibraryCardDetail>('library.card', { deck, path })
}

export interface SaveCardRequest {
  deck: string
  path: string
  view: string
  base: string
  from?: { deck: string; path: string }
}

/** Save a card; a mutation, so both card lists are refreshed. The card list
 *  is awaited so a host can re-open the saved card at its new path. */
export async function saveCard(req: SaveCardRequest): Promise<LibraryCardDetail> {
  const detail = await rumu.request<LibraryCardDetail>('library.save', { ...req })
  await refreshLibrary()
  await refreshCards()
  return detail
}

export async function deleteCard(deck: string, path: string, base: string): Promise<void> {
  await rumu.request('library.delete', { deck, path, base })
  await refreshLibrary()
  void refreshCards()
}

/** Create a deck and return it. */
export async function createDeck(name: string): Promise<LibraryDeck> {
  const deck = await rumu.request<LibraryDeck>('library.deck_create', { name })
  await refreshLibrary()
  return deck
}

export async function renameDeck(id: string, name: string): Promise<LibraryDeck> {
  const deck = await rumu.request<LibraryDeck>('library.deck_rename', { id, name })
  await refreshLibrary()
  return deck
}

/** Delete a deck and every card in it (assistant decks are refused). */
export async function deleteDeck(id: string): Promise<void> {
  await rumu.request('library.deck_delete', { id })
  if (openedCard.value?.deck === id) moveOpenCard(null)
  dropFolders(id, null)
  await refreshLibrary()
  void refreshCards()
}

/** A folder key as stored in `state.library.folders`. */
const folderKey = (deck: string, path: string): string => `${deck}:${path}`

/** True when `path` is `prefix` itself or inside it. */
const within = (path: string, prefix: string): boolean => path === prefix || path.startsWith(`${prefix}/`)

/** Forget the empty folders of a deck at or under `prefix` (all of them for
 *  `null`). */
function dropFolders(deck: string, prefix: string | null): void {
  for (const key of [...state.library.folders]) {
    if (!key.startsWith(`${deck}:`)) continue
    if (prefix === null || within(key.slice(deck.length + 1), prefix)) state.library.folders.delete(key)
  }
}

/** Add an empty folder to the tree; it lasts until a card lands in it or the
 *  page reloads. */
export function addFolder(deck: string, path: string): void {
  state.library.folders.add(folderKey(deck, path))
  state.library.expanded.add(`dir:${deck}:${path}`)
}

/** Rename or move a card, or a folder with everything in it, within a deck.
 *  The open card, expanded folders and empty folders follow it. */
export async function moveEntry(deck: string, from: string, to: string, folder: boolean): Promise<void> {
  if (from === to) return
  const remap = (path: string): string | null => {
    if (!folder) return path === from ? to : null
    return within(path, from) ? `${to}${path.slice(from.length)}` : null
  }
  const hasCards = state.library.cards.some(
    (card) =>
      card.deck === deck && (folder ? within(card.path, from) && card.path !== from : card.path === from),
  )
  if (hasCards) await rumu.request('library.move', { deck, from, to, folder })
  if (folder) {
    for (const key of [...state.library.folders]) {
      const next = key.startsWith(`${deck}:`) ? remap(key.slice(deck.length + 1)) : null
      if (next === null) continue
      state.library.folders.delete(key)
      state.library.folders.add(folderKey(deck, next))
    }
    for (const key of [...state.library.expanded]) {
      const prefix = `dir:${deck}:`
      const next = key.startsWith(prefix) ? remap(key.slice(prefix.length)) : null
      if (next === null) continue
      state.library.expanded.delete(key)
      state.library.expanded.add(`${prefix}${next}`)
    }
  }
  const open = openedCard.value
  if (open && !open.isNew && open.deck === deck) {
    const next = remap(open.path)
    if (next !== null) moveOpenCard({ deck, path: next })
  }
  if (!hasCards) return
  await refreshLibrary()
  void refreshCards()
}

/** Delete a folder and every card under it. */
export async function deleteFolder(deck: string, path: string): Promise<void> {
  const hasCards = state.library.cards.some((card) => card.deck === deck && card.path.startsWith(`${path}/`))
  if (hasCards) await rumu.request('library.delete_folder', { deck, path })
  dropFolders(deck, path)
  const open = openedCard.value
  if (open?.deck === deck && open.path.startsWith(`${path}/`)) moveOpenCard(null)
  if (!hasCards) return
  await refreshLibrary()
  void refreshCards()
}

export function cardHistory(deck: string, path: string): Promise<LibraryCommit[]> {
  return rumu.request<LibraryCommit[]>('library.history', { deck, path })
}

export function cardVersion(deck: string, path: string, commit: string): Promise<LibraryVersion> {
  return rumu.request<LibraryVersion>('library.version', { deck, path, commit })
}
