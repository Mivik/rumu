// URL ↔ what the chat's sheet shows: a session, or one of the management
// pages.
//
// The History API directly rather than vue-router. An open session owns
// `/s/<id>`, a view other than the conversation adds `?view=<name>`, and the
// landing page is `/`. A page owns `/<page>`, the item open on it follows
// (`/decks/<deck>/<path>`, `/workspaces/<name>`, …), and `?new` is its
// creation form. The rumu server already serves index.html for unknown paths
// (SPA fallback), so a refresh boots into whatever the address names.
//
// The address is derived from the state (`currentUrl`): a mutation sets the
// state, then commits it as a new entry (a navigation) or over the current one
// (a correction, such as a card that moved).

import { state } from './state/state'

const SESSION_PATH = /^\/s\/([^/]+)\/?$/

/** The ways to look at an open session: the conversation, the raw
 *  trajectory of its nodes, or its usage statistics. */
export type SessionView = 'chat' | 'trace' | 'stats'

/** Replies the stats view narrows to, by their first model call's node id
 *  (`?from=&to=`); `to` may equal `from`. */
export interface StatsRange {
  from: string
  to: string
}

export const PAGES = ['decks', 'workspaces', 'assistants', 'projects', 'settings'] as const
export type Page = (typeof PAGES)[number]

/** A page and the item open on it: a card (`<deck>/<path>`, a deck alone for
 *  its root), a workspace name, an assistant or project id, a settings
 *  section. `fresh` opens the page's creation form (in the library, at the
 *  item's folder). */
export interface PageRoute {
  page: Page
  item: string | null
  fresh: boolean
}

function decode(segment: string): string | null {
  try {
    return decodeURIComponent(segment)
  } catch {
    return null
  }
}

/** The session named by the current address, if any. */
export function sessionIdFromLocation(): string | null {
  const m = window.location.pathname.match(SESSION_PATH)
  return m ? decode(m[1]!) : null
}

/** The view named by the current address (the conversation when none is). */
export function viewFromLocation(): SessionView {
  const v = new URLSearchParams(window.location.search).get('view')
  return v === 'trace' || v === 'stats' ? v : 'chat'
}

/** The stats range named by the current address, if any. */
export function rangeFromLocation(): StatsRange | null {
  const q = new URLSearchParams(window.location.search)
  const from = q.get('from')
  return q.get('view') === 'stats' && from ? { from, to: q.get('to') ?? from } : null
}

/** The page named by the current address, if any. */
export function pageFromLocation(): PageRoute | null {
  const [, head, ...rest] = window.location.pathname.split('/')
  const page = PAGES.find((p) => p === head)
  if (!page) return null
  const segments = rest.map(decode)
  const item = segments.includes(null) ? null : segments.join('/')
  return { page, item: item || null, fresh: new URLSearchParams(window.location.search).has('new') }
}

function currentUrl(): string {
  const p = state.page
  if (p) {
    const item = p.item ? `/${p.item.split('/').map(encodeURIComponent).join('/')}` : ''
    return `/${p.page}${item}${p.fresh ? '?new' : ''}`
  }
  const id = state.sessionId
  if (!id) return '/'
  if (state.view === 'chat') return `/s/${encodeURIComponent(id)}`
  const q = new URLSearchParams({ view: state.view })
  const range = state.view === 'stats' ? state.statsRange : null
  if (range) {
    q.set('from', range.from)
    if (range.to !== range.from) q.set('to', range.to)
  }
  return `/s/${encodeURIComponent(id)}?${q}`
}

/** Write the state into the address bar (no-op when it already matches).
 *  `up` marks an entry pushed from its page's list, so leaving the item can
 *  go Back to it rather than stack another list entry. */
export function commitRoute(mode: 'push' | 'replace' = 'push', up = false): void {
  const url = currentUrl()
  if (window.location.pathname + window.location.search === url) return
  if (mode === 'replace') window.history.replaceState({ up: window.history.state?.up ?? false }, '', url)
  else window.history.pushState({ up }, '', url)
}

/** The sheet a page shows on: workspaces share the projects page. */
export function sheetOf(page: Page): Page {
  return page === 'workspaces' ? 'projects' : page
}

/** Navigate to a page, at `item` when given. */
export function openPage(page: Page, item: string | null = null, fresh = false): void {
  const from = state.page
  const up = !!from && sheetOf(from.page) === sheetOf(page) && !from.item && !from.fresh && (!!item || fresh)
  state.page = { page, item, fresh }
  commitRoute('push', up)
}

/** Correct the open page's item in place (it was renamed, created, deleted). */
export function replacePage(item: string | null, fresh = false): void {
  if (!state.page) return
  state.page = { page: state.page.page, item, fresh }
  commitRoute('replace')
}

/** Leave the open item for the page's list: Back when the list is the entry
 *  it was opened from. */
export function pageUp(): void {
  if (!state.page) return
  if (window.history.state?.up) window.history.back()
  else replacePage(null)
}

/** Back to the open session (or the landing page). */
export function closePage(): void {
  if (!state.page) return
  state.page = null
  commitRoute('push')
}
