// Following links in rendered markdown (lib/markdown.ts makes the anchors):
// `@rumu:` addresses open what they name, links inside a card body open cards
// of the same deck, a workspace file opens beside the chat (its latest `show`
// on this path, else as it is now), and an address nothing here shows is
// copied instead.

import { t } from '../i18n'
import { rumu } from './api'
import { artifactsOnPath, showArtifact } from './artifacts'
import { cardById, mountedRef } from './cards'
import { closePage, openPage } from './router'
import {
  closeDrawer,
  openCard,
  openCardWindow,
  openDrawer,
  openLibrary,
  openLibraryAtDeck,
  openSession,
  openTaskDetail,
  state,
} from './store'
import { toast } from './state/ui'
import type { Page } from './router'
import type { Artifact, LibraryRef } from './types'
import { copyText } from './util'

/** Open a card by deck and deck-local path. A card window stays put: the
 *  link opens its target in a new window above it. The library page goes to
 *  the card (Back returns). Elsewhere a mounted card opens in the drawer and
 *  an unmounted one in the library. */
function openDeckCard(ref: LibraryRef): void {
  if (state.drawers.length) return openCardWindow(ref.deck, ref.path)
  if (state.page?.page === 'decks') return openCard(ref.deck, ref.path)
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
      // `/sessions/by-assistant/<deck>`: the assistant.
      if (sid === 'by-assistant') {
        const id = segs[1]
        return go(
          'assistants',
          id ?? null,
          state.assistants.some((a) => a.id === id),
        )
      }
      if (!sid) return false
      closeDrawer()
      // A failed open reports itself; a page gives way once it loads.
      void openSession(sid)
      return true
    }
    case 'tasks': {
      const id = segs[0]
      if (!id || !state.tasks[id]) return false
      closeDrawer()
      closePage()
      if (state.taskDetail !== id) openTaskDetail(id)
      return true
    }
    case 'workspaces':
      return go(
        'workspaces',
        segs[0] ?? null,
        state.workspaces.some((w) => w.name === segs[0]),
      )
    // `/decks/<name>`: an assistant's or a project's page, or the deck in
    // the library.
    case 'decks': {
      const name = segs[0]
      const assistant = state.assistants.find((a) => a.name === name)
      if (assistant) return go('assistants', assistant.id)
      const project = state.projects.find((p) => p.name === name)
      if (project) return go('projects', project.id)
      return go('decks', null)
    }
    case 'skills':
      return go('settings', 'skills')
    case 'mcp':
      return go('settings', 'mcp')
  }
  return false
}

/** Open a page at `item` when it names one, else the page's list. */
function go(page: Page, item: string | null, known = true): true {
  closeDrawer()
  if (item && known) openPage(page, item)
  else openPage(page === 'workspaces' ? 'projects' : page)
  return true
}

/** `ws:path`: the latest snapshot of it shown on this path, else the file as
 *  it is now; `ws:` alone: the registered workspace. False when none. */
function openWorkspaceAddress(address: string): boolean {
  const versions = artifactsOnPath.value.get(address.replace(/:\.\//, ':'))
  if (versions?.length) {
    closeDrawer()
    closePage()
    showArtifact(versions.at(-1)!.node.id)
    return true
  }
  const m = /^([^:]+):\/?$/.exec(address)
  if (m) return state.workspaces.some((w) => w.name === m[1]) && go('workspaces', m[1]!)
  const branch = state.activeBranch
  if (!branch) return false
  rumu.request<{ artifact: Artifact }>('fs.snapshot', { branch, path: address }).then(
    ({ artifact }) => {
      closeDrawer()
      closePage()
      showArtifact(artifact)
    },
    (err) => toast('error', t('chat.artifact.openFailed', { path: address }), (err as Error).message),
  )
  return true
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
  } else if (wsRef !== undefined) {
    if (!openWorkspaceAddress(wsRef)) void copyAddress(wsRef)
  } else return false
  return true
}
