// Where you left a session's main line, so a reload opens there instead of
// at the end. Kept per tab (sessionStorage): a reload restores it, a new tab
// opens at the end. A view at the end keeps no place.

import { shallowRef } from 'vue'
import type { NodeId, SessionId } from '../types'

/** An entry (by its data-node-id) and how far its top sat below the
 *  scroller's top, px. */
export interface ScrollPlace {
  node: NodeId
  offset: number
}

const KEY = 'rumu.scrollPlace'
/** Sessions remembered at most, the oldest dropped first. */
const KEEP = 50

function places(): Record<SessionId, ScrollPlace> {
  try {
    return JSON.parse(sessionStorage.getItem(KEY) ?? '{}') as Record<SessionId, ScrollPlace>
  } catch {
    return {}
  }
}

/** Remember where `session` is read, or (null) that it is read at its end. */
export function rememberPlace(session: SessionId, place: ScrollPlace | null): void {
  const all = places()
  delete all[session]
  if (place) all[session] = place
  const keys = Object.keys(all)
  for (const k of keys.slice(0, Math.max(0, keys.length - KEEP))) delete all[k]
  try {
    sessionStorage.setItem(KEY, JSON.stringify(all))
  } catch {
    // storage full or blocked: the place is only a convenience
  }
}

/** The place the open in progress lands on, until the view has taken it. */
export const pendingPlace = shallowRef<(ScrollPlace & { session: SessionId }) | null>(null)

let loaded = false

/** The first open after the page loads: `session`'s remembered place, which
 *  its view then opens at (`pendingPlace`). Later opens land at the end. */
export function takePlace(session: SessionId): ScrollPlace | null {
  if (loaded) return null
  loaded = true
  const place = places()[session] ?? null
  pendingPlace.value = place ? { ...place, session } : null
  return place
}
