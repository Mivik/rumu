// The main line's context use, for the composer's meter: how much of the
// next model's window the conversation fills, and where it compacts. Mirrors
// the kernel's `estimate` / `threshold` (crates/rumu-kernel/src/compact.rs):
// the last reply's measured usage, plus a rough count of what came after it.

import { computed } from 'vue'
import type { Content, Node } from '../types'
import { textTokens } from '../util'
import { pathNodes } from './derived'
import { currentPick, models, resolvePick } from './models'
import { state } from './state'
import { compactionOf } from './timeline'

/** As the kernel: the window assumed when the catalog does not know it, the
 *  share it compacts at, the room kept for a reply, tokens per image. */
const UNKNOWN_WINDOW = 128_000
const RATIO = 0.85
const RESERVE_REPLY = 20_000
const RESERVE_BUFFER = 13_000
const IMAGE_TOKENS = 1_200

function contentTokens(c: Content): number {
  return textTokens(c.text) + (c.attachments?.length ?? 0) * IMAGE_TOKENS
}

function nodeTokens(n: Node): number {
  const b = n.body
  switch (b.type) {
    case 'user':
    case 'note':
      return contentTokens(b.content)
    case 'tool_result':
      return b.invoke ? 0 : contentTokens(b.content)
    case 'assistant':
      return (
        contentTokens(b.content) +
        b.tool_calls.reduce((s, c) => s + textTokens(c.name) + textTokens(JSON.stringify(c.args)), 0)
      )
  }
}

export interface ContextUse {
  /** Tokens the next request carries, measured plus estimated. */
  used: number
  /** The part of `used` not measured yet (what came after the last reply,
   *  and the reply streaming now). */
  estimated: number
  window: number
  /** The catalog does not know the window: `window` is the kernel's guess. */
  assumed: boolean
  /** Where it compacts on its own; null when auto-compaction is off. */
  threshold: number | null
}

export const contextUse = computed<ContextUse | null>(() => {
  const branch = state.mainBranch
  const s = models.settings
  if (!branch || !s) return null
  const model = resolvePick(currentPick())?.model
  const window = model?.context ?? UNKNOWN_WINDOW

  const path = pathNodes.value
  let base = 0
  let from = 0
  for (let i = path.length - 1; i >= 0; i--) {
    const n = path[i]!
    if (n.body.type === 'assistant' && n.body.usage.prompt_tokens > 0) {
      base = n.body.usage.prompt_tokens + n.body.usage.completion_tokens
      from = i + 1
      break
    }
    // Replies before a compaction were measured on the uncompacted view;
    // the note records the estimate it left.
    const c = compactionOf(n)
    if (c) {
      base = c.after
      from = i + 1
      break
    }
  }
  let estimated = path.slice(from).reduce((sum, n) => sum + nodeTokens(n), 0)
  estimated += state.deltas[branch]?.tokens ?? 0

  let threshold: number | null = null
  if (s.compact.auto) {
    const reserve = Math.min(model?.output ?? RESERVE_REPLY, RESERVE_REPLY) + RESERVE_BUFFER
    threshold = Math.min(Math.max(window - reserve, 0), Math.floor(window * RATIO))
    const at = s.compact.at
    if (at && at > 0) threshold = Math.min(threshold, Math.floor(at <= 1 ? window * at : at))
  }
  return { used: base + estimated, estimated, window, assumed: model?.context == null, threshold }
})
