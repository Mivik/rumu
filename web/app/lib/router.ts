// URL ↔ open session and its view.
//
// The History API directly rather than vue-router: an open session owns
// `/s/<id>`, a view other than the conversation adds `?view=<name>`, and the
// landing page is `/`. The rumu server already serves index.html for unknown
// paths (SPA fallback), so a refresh on a session URL boots into that session
// and view.

const SESSION_PATH = /^\/s\/([^/]+)\/?$/

/** The ways to look at an open session: the conversation, or the raw
 *  trajectory of its nodes. */
export type SessionView = 'chat' | 'trace'

/** The session named by the current address, if any. */
export function sessionIdFromLocation(): string | null {
  const m = window.location.pathname.match(SESSION_PATH)
  if (!m) return null
  try {
    return decodeURIComponent(m[1])
  } catch {
    return null
  }
}

/** The view named by the current address (the conversation when none is). */
export function viewFromLocation(): SessionView {
  return new URLSearchParams(window.location.search).get('view') === 'trace' ? 'trace' : 'chat'
}

/** Write the open session and view into the address bar (no-op when it
 *  already matches). */
export function routeSession(id: string | null, view: SessionView, mode: 'push' | 'replace' = 'push'): void {
  const url = id ? `/s/${encodeURIComponent(id)}${view === 'chat' ? '' : `?view=${view}`}` : '/'
  if (window.location.pathname + window.location.search === url) return
  const state = { session: id }
  if (mode === 'replace') window.history.replaceState(state, '', url)
  else window.history.pushState(state, '', url)
}
