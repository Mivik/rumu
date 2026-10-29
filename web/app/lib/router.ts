// URL ↔ open session.
//
// The app has one view, so this is the History API directly rather than
// vue-router: an open session owns `/s/<id>` and the landing page is `/`.
// The rumu server already serves index.html for unknown paths (SPA fallback),
// so a refresh on a session URL boots into that session.

const SESSION_PATH = /^\/s\/([^/]+)\/?$/

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

/** Write the open session into the address bar (no-op when it already matches). */
export function routeSession(id: string | null, mode: 'push' | 'replace' = 'push'): void {
  if (sessionIdFromLocation() === id) return
  const url = id ? `/s/${encodeURIComponent(id)}` : '/'
  const state = { session: id }
  if (mode === 'replace') window.history.replaceState(state, '', url)
  else window.history.pushState(state, '', url)
}
