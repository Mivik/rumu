// Streaming buffers: the `llm.delta` pushes of a model call in flight, and
// the `session.tree` snapshot that lets a client joining midway (a reload, a
// reconnect) pick a running turn back up.

import type { BranchId, LiveTurn, LlmDeltaPush, SessionTreeResult } from '../types'
import { textTokens } from '../util'
import { MAX_DELTA_CHARS, state, type DeltaBuffer } from './state'

/** The part of a chunk starting at `at` that lies past `len`, what is held. */
function unseen(chunk: string | undefined, at: number | undefined, len: number): string {
  if (!chunk) return ''
  const skip = len - (at ?? len)
  return skip > 0 ? chunk.slice(skip) : chunk
}

function fold(buf: DeltaBuffer, p: LlmDeltaPush): void {
  buf.seq = p.seq
  const reasoning = unseen(p.reasoning, p.at, buf.reasoningLen)
  const text = unseen(p.text, p.at, buf.textLen)
  buf.tokens += textTokens(reasoning) + textTokens(text)
  if (reasoning) {
    buf.reasonStart ??= Date.now()
    buf.reasoning = (buf.reasoning + reasoning).slice(-MAX_DELTA_CHARS)
    buf.reasoningLen += reasoning.length
    buf.hasReasoning = true
  }
  if (text) {
    if (buf.reasonStart !== undefined && buf.reasonMs === undefined) {
      buf.reasonMs = Date.now() - buf.reasonStart
    }
    buf.text = (buf.text + text).slice(-MAX_DELTA_CHARS)
    buf.textLen += text.length
    buf.hasText = true
  }
}

/** Fold one delta into its branch's buffer. A delta already covered (by the
 *  snapshot) is skipped, and the covered part of a merged one dropped; a
 *  stream first seen midway keeps its deltas until the snapshot supplies
 *  the start. */
export function applyDelta(p: LlmDeltaPush): void {
  let buf = state.deltas[p.branch]
  if (!buf) {
    buf = {
      text: '',
      reasoning: '',
      hasReasoning: false,
      hasText: false,
      seq: 0,
      textLen: 0,
      reasoningLen: 0,
      tokens: 0,
    }
    if ((p.from ?? p.seq) > 1) buf.early = []
    state.deltas[p.branch] = buf
  }
  if (p.seq <= buf.seq) return
  buf.early?.push(p)
  fold(buf, p)
}

function fromSnapshot(live: LiveTurn, early: LlmDeltaPush[]): DeltaBuffer {
  const buf: DeltaBuffer = {
    text: live.text,
    reasoning: live.reasoning,
    hasReasoning: !!live.reasoning,
    hasText: !!live.text,
    seq: live.seq,
    textLen: live.text.length,
    reasoningLen: live.reasoning.length,
    tokens: textTokens(live.reasoning) + textTokens(live.text),
  }
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
      continue
    }
    const prev = state.deltas[id]
    // A buffer that saw its whole stream and is ahead of the snapshot stays.
    if (prev && !prev.early && prev.seq >= live.seq) continue
    if (live.seq || prev?.early) state.deltas[id] = fromSnapshot(live, prev?.early ?? [])
  }
}
