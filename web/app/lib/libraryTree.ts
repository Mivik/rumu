// The library page's tree: decks → folders → cards, built client-side from
// the flat card rows. Deck and folder children put folders first, then cards,
// both by name.

import type { LibraryCard, LibraryDeck } from './types'

export type TreeNode =
  | { kind: 'deck'; key: string; deck: LibraryDeck; children: TreeNode[] }
  | { kind: 'folder'; key: string; deck: string; path: string; name: string; children: TreeNode[] }
  | { kind: 'card'; key: string; card: LibraryCard }

interface Folder {
  /** The folder's own name (`""` for a deck root). */
  name: string
  /** The path from the deck root (`""` for a deck root). */
  path: string
  folders: Map<string, Folder>
  cards: LibraryCard[]
}

const cardName = (card: LibraryCard): string => card.path.split('/').at(-1) || card.path

function children(folder: Folder, deckId: string): TreeNode[] {
  const folders: TreeNode[] = [...folder.folders.values()]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((child) => ({
      kind: 'folder',
      key: `dir:${deckId}:${child.path}`,
      deck: deckId,
      path: child.path,
      name: child.name,
      children: children(child, deckId),
    }))
  const cards: TreeNode[] = [...folder.cards]
    .sort((a, b) => cardName(a).localeCompare(cardName(b)))
    .map((card) => ({ kind: 'card', key: `card:${deckId}:${card.path}`, card }))
  return [...folders, ...cards]
}

/** Build the deck roots from the cards given (search passes only the hits)
 *  and the empty folders (`{deck, path}`) made in the tree. */
export function buildLibraryTree(
  decks: LibraryDeck[],
  cards: LibraryCard[],
  folders: { deck: string; path: string }[] = [],
): TreeNode[] {
  const roots = new Map<string, Folder>()
  /** The folder at `segments` under a deck root, made on the way. */
  const folderAt = (deck: string, segments: string[]): Folder => {
    let folder = roots.get(deck)
    if (!folder) {
      folder = { name: '', path: '', folders: new Map(), cards: [] }
      roots.set(deck, folder)
    }
    for (const segment of segments) {
      const path: string = folder.path ? `${folder.path}/${segment}` : segment
      let child = folder.folders.get(segment)
      if (!child) {
        child = { name: segment, path, folders: new Map(), cards: [] }
        folder.folders.set(segment, child)
      }
      folder = child
    }
    return folder
  }
  for (const card of cards) folderAt(card.deck, card.path.split('/').slice(0, -1)).cards.push(card)
  for (const { deck, path } of folders) folderAt(deck, path.split('/'))
  return [...decks]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((deck) => ({
      kind: 'deck' as const,
      key: `deck:${deck.id}`,
      deck,
      children: roots.has(deck.id) ? children(roots.get(deck.id)!, deck.id) : [],
    }))
}
