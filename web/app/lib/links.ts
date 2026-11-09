// Following links in rendered markdown (lib/markdown.ts makes the anchors):
// `@rumu:` addresses open what they name, links inside a card body open cards
// of the same deck, and a workspace address rumu cannot open (`@scratch:`,
// `proj:`) is copied instead.

import { t } from '../i18n'
import { cardById, mountedRef } from './cards'
import {
  closeDrawer,
  closeLibrary,
  openCardWindow,
  openDrawer,
  openLibrary,
  openLibraryAtDeck,
  openSession,
  state,
} from './store'
import { toast } from './state/ui'
import type { LibraryRef } from './types'
import { copyText } from './util'

/** Open a card by deck and deck-local path. A card already on screen stays
 *  put: the link opens its target in a new window above it. With nothing
 *  showing, a mounted card opens in the drawer and an unmounted one in the
 *  library. */
function openDeckCard(ref: LibraryRef): void {
  if (state.library.isOpen || state.drawers.length) return openCardWindow(ref.deck, ref.path)
  const mounted = state.cards.find((c) => c.deck === ref.deck && c.path === ref.path)
  if (mounted) return openDrawer(mounted.id)
  closeDrawer()
  openLibrary(ref.deck, ref.path)
}

/** `/cards/<mount>/<path>`: a card, or a folder (trailing `/`, a mount point,
 * a prefix of mounted cards) opened at its deck in the library. */
function openCardAddress(rest: string): boolean {
  const id = rest
    .replace(/[?#].*$/, '')
    .replace(/\/+$/, '')
    .replace(/\.md$/, '')
  if (!id) {
    closeDrawer()
    openLibrary()
    return true
  }
  const ref = mountedRef(id)
  if (!ref) return false
  const folder =
    rest.endsWith('/') ||
    !ref.path ||
    (!cardById.value.has(id) && state.cards.some((c) => c.id.startsWith(`${id}/`)))
  if (!folder) {
    openDeckCard(ref)
    return true
  }
  closeDrawer()
  openLibraryAtDeck(ref.deck)
  return true
}

/** Open what an `@rumu` path names; false when nothing here shows it. */
function openAddress(abs: string): boolean {
  const [head, ...segs] = abs.slice(1).split('/')
  switch (head) {
    case 'cards':
      return openCardAddress(segs.join('/'))
    // `/history/cards/<id>[@<commit>]`: the card itself (its history is a
    // click away there).
    case 'history':
      return (
        segs[0] === 'cards' &&
        openCardAddress(
          segs
            .slice(1)
            .join('/')
            .replace(/@[^/]*$/, ''),
        )
      )
    case 'sessions': {
      const sid = segs[0]
      if (!sid || sid === 'by-assistant') return false
      closeDrawer()
      closeLibrary()
      // A failed open reports itself.
      void openSession(sid)
      return true
    }
    case 'tasks': {
      const id = segs[0]
      if (!id || !state.tasks[id]) return false
      closeDrawer()
      closeLibrary()
      state.taskDetail = id
      return true
    }
  }
  return false
}

async function copyAddress(address: string): Promise<void> {
  if (await copyText(address)) toast('info', t('common.copiedAddress'), address)
  else toast('error', t('common.copyFailed'))
}

/** Follow a rendered-markdown anchor; false when it is an ordinary link. */
export function followLink(link: HTMLAnchorElement): boolean {
  const { rumu: abs, cardDeck, cardPath, wsRef } = link.dataset
  if (cardDeck && cardPath) openDeckCard({ deck: cardDeck, path: cardPath })
  else if (abs !== undefined) {
    if (!openAddress(abs)) void copyAddress(`@rumu:${abs}`)
  } else if (wsRef !== undefined) void copyAddress(wsRef)
  else return false
  return true
}
