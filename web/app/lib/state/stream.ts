// Streaming buffers: the `llm.delta` pushes of a model call in flight, and
// the `session.tree` snapshot that lets a client joining midway (a reload, a
// reconnect) pick a running turn back up.

import type { BranchId, LiveTurn, LlmDeltaPush, SessionTreeResult } from '../types'
import { MAX_DELTA_CHARS, state, type DeltaBuffer } from './state'

function fold(buf: DeltaBuffer, p: LlmDeltaPush): void {
  buf.seq = p.seq
  // Marks flip at most twice per stream; the text itself changes on every
  // delta, and anything that reads it rebuilds on every delta too.
  let marks = state.streamMarks[p.branch]
  if (!marks) marks = state.streamMarks[p.branch] = { reasoning: false, text: false }
  if (p.reasoning && !marks.reasoning) marks.reasoning = true
  if (p.text && !marks.text) marks.text = true
  if (p.reasoning) {
    buf.reasonStart ??= Date.now()
    buf.reasoning = (buf.reasoning + p.reasoning).slice(-MAX_DELTA_CHARS)
  }
  if (p.text) {
    if (buf.reasonStart !== undefined && buf.reasonMs === undefined) {
      buf.reasonMs = Date.now() - buf.reasonStart
    }
    buf.text = (buf.text + p.text).slice(-MAX_DELTA_CHARS)
  }
}

/** Fold one delta into its branch's buffer. A delta already covered (by the
 *  snapshot) is skipped; a stream first seen midway keeps its deltas until
 *  the snapshot supplies the start. */
export function applyDelta(p: LlmDeltaPush): void {
  let buf = state.deltas[p.branch]
  if (!buf) {
    buf = { text: '', reasoning: '', seq: 0 }
    if (p.seq > 1) buf.early = []
    state.deltas[p.branch] = buf
    state.streamMarks[p.branch] = { reasoning: false, text: false }
  }
  if (p.seq <= buf.seq) return
  buf.early?.push(p)
  fold(buf, p)
}

function fromSnapshot(live: LiveTurn, early: LlmDeltaPush[]): DeltaBuffer {
  const buf: DeltaBuffer = { text: live.text, reasoning: live.reasoning, seq: live.seq }
  // The thinking clock restarts at the join; a reply already under way
  // means thinking ended at some unknown time before it.
  if (live.reasoning) buf.reasonStart = Date.now()
  if (live.reasoning && live.text) buf.reasonMs = 0
  for (const p of early) if (p.seq > buf.seq) fold(buf, p)
  return buf
}

/** Sync the busy flags and streaming buffers of a freshly loaded tree's
 *  branches with the server's running turns. */
export function syncLive(tree: SessionTreeResult): void {
  for (const b of tree.branches) {
    const id: BranchId = b.id
    const live = tree.live[id]
    state.busy[id] = !!live
    if (!live) {
      delete state.deltas[id]
      delete state.streamMarks[id]
      continue
    }
    state.streamMarks[id] = { reasoning: !!live.reasoning, text: !!live.text }
    const prev = state.deltas[id]
    // A buffer that saw its whole stream and is ahead of the snapshot stays.
    if (prev && !prev.early && prev.seq >= live.seq) continue
    if (live.seq || prev?.early) state.deltas[id] = fromSnapshot(live, prev?.early ?? [])
  }
}
