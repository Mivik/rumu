// Live output of background tasks, fetched only while something shows it.
//
// The server sends a task's output appends only to a client that watches
// the task (`task.watch { ids }`); a newly watched task comes with its
// buffered output, and later appends (gathered, at most every 100ms) go on
// from there. Views say which tasks they show with `useTaskWatch`; the
// union of them is what this client watches. Buffers live outside the
// reactive state (a flood would make every append a deep update), and a
// per-task revision tells readers to re-read.

import { onScopeDispose, reactive, watch } from 'vue'
import { rumu } from '../api'
import type { TaskId } from '../types'

/** Live output shown (chars): its tail. The server's gathered appends
 * (`PROGRESS_TAIL` in ws.rs) always cover it. */
export const LIVE_CHARS = 64_000
/** Live output kept: more than is shown, so a view can tell it is clipped. */
const KEEP_CHARS = 2 * LIVE_CHARS

const buffers = new Map<TaskId, string>()
const revs = reactive<Record<TaskId, number>>({})
/** How many views show each task. */
const wanted = new Map<TaskId, number>()
/** The set last sent on this connection, sorted and joined; `null` before
 *  the first (until then the server sends every task's appends). */
let sent: string | null = null
let scheduled = false

function bump(id: TaskId): void {
  revs[id] = (revs[id] ?? 0) + 1
}

/** Keep the tail of `text`: the last `max` chars from a line start when one
 * is near. */
export function clipLive(text: string, max = LIVE_CHARS): { text: string; clipped: boolean } {
  if (text.length <= max) return { text, clipped: false }
  const tail = text.slice(-max)
  const nl = tail.indexOf('\n')
  return { text: nl >= 0 && nl < 1000 ? tail.slice(nl + 1) : tail, clipped: true }
}

/** A watched task's live output (its tail, to `clipLive`); '' when not
 *  watched. */
export function taskOutput(id: TaskId | null): string {
  if (!id) return ''
  void revs[id]
  return buffers.get(id) ?? ''
}

/** `task.progress` with an `append`: extend the buffer of a watched task. */
export function appendTaskOutput(id: TaskId, append: string): void {
  const prev = buffers.get(id)
  if (prev === undefined) return
  const next = prev + append
  // Trimmed in steps, not on every append.
  buffers.set(id, next.length > 2 * KEEP_CHARS ? next.slice(-KEEP_CHARS) : next)
  bump(id)
}

function schedule(): void {
  if (scheduled) return
  scheduled = true
  queueMicrotask(() => {
    scheduled = false
    void sync()
  })
}

async function sync(): Promise<void> {
  const ids = [...wanted.keys()].sort()
  const key = ids.join('\n')
  if (key === sent) return
  sent = key
  for (const id of buffers.keys()) {
    if (!wanted.has(id)) {
      buffers.delete(id)
      delete revs[id]
    }
  }
  try {
    const r = await rumu.request<{ output: Record<TaskId, string> }>('task.watch', { ids })
    for (const [id, text] of Object.entries(r.output)) {
      if (!wanted.has(id)) continue
      buffers.set(id, text.slice(-KEEP_CHARS))
      bump(id)
    }
  } catch {
    // Not connected: the reconnect sends the set again.
    sent = null
  }
}

/** A new connection watches nothing yet: send the set again and take
 *  fresh buffers. */
export function resyncTaskWatch(): void {
  sent = null
  for (const id of [...buffers.keys()]) buffers.delete(id)
  schedule()
}

function hold(id: TaskId): void {
  wanted.set(id, (wanted.get(id) ?? 0) + 1)
}

function release(id: TaskId): void {
  const n = (wanted.get(id) ?? 0) - 1
  if (n > 0) wanted.set(id, n)
  else wanted.delete(id)
}

/** Watch the tasks `ids` names while the calling component lives. */
export function useTaskWatch(ids: () => TaskId[]): void {
  let held: TaskId[] = []
  watch(
    () => ids().join('\n'),
    (key) => {
      const next = key ? key.split('\n') : []
      next.forEach(hold)
      held.forEach(release)
      held = next
      schedule()
    },
    { immediate: true },
  )
  onScopeDispose(() => {
    held.forEach(release)
    held = []
    schedule()
  })
}
