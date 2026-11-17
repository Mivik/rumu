// Usage statistics over a path: its replies (one per timeline group — split
// at user prompts and compactions, so a wake by a finished task stays in the
// reply it continues) and their model calls, and the sums a range of them
// adds up to. Pure derivation; the stats view and the per-reply usage card
// both read it.

import type { Node, NodeId } from '../types'
import { TURN_ERROR_TAG } from '../types'
import { compactionOf } from './timeline'

type AsstBody = Extract<Node['body'], { type: 'assistant' }>

/** One model call: an Assistant node. */
export interface CallStat {
  node: Node
  /** 1-based position among the replies of the path. */
  reply: number
  model: string
  input: number
  cached: number
  written: number
  output: number
  durMs: number | null
  ttftMs: number | null
  reasonMs: number | null
  retries: number
  reset: boolean
  /** Tool names it called; empty for a call that only answered. */
  tools: string[]
}

/** One reply: the model calls and tool runs answering a prompt (or a wake). */
export interface ReplyStat {
  /** 1-based along the path. */
  index: number
  /** Its first model call: the reply's identity in links. */
  id: NodeId
  /** The prompt it answers; null for a reply opened by a wake or a compaction. */
  prompt: Node | null
  calls: CallStat[]
  modelMs: number
  toolMs: number
  /** Time between the steps (hooks, recall, retry backoff…), idle waits excluded. */
  otherMs: number
  /** Turns that failed inside it (a recorded `turn.error`). */
  failures: number
}

export interface Summary {
  replies: number
  calls: number
  /** wait + gen + tool + other. */
  activeMs: number
  /** Model calls before their first token (queue, prefill, network). Calls
   *  recorded without a first-token time count wholly as generation. */
  waitMs: number
  /** Model calls from their first token on (reasoning and output). */
  genMs: number
  toolMs: number
  otherMs: number
  input: number
  cached: number
  written: number
  output: number
  /** cached / input, 0–1; null without input. */
  hitRate: number | null
  avgTtftMs: number | null
  maxTtftMs: number | null
  /** Output tokens per second of generation (after the first token where known). */
  rate: number | null
  retries: number
  failures: number
  resets: number
  /** Models by call count, most used first. */
  models: { ref: string; calls: number }[]
  /** The context of the range's last call. */
  lastInput: number
}

function callOf(n: Node, reply: number): CallStat {
  const b = n.body as AsstBody
  return {
    node: n,
    reply,
    model: b.model,
    input: b.usage.prompt_tokens,
    cached: b.usage.cached_tokens,
    written: b.usage.cache_creation_tokens,
    output: b.usage.completion_tokens,
    durMs: n.dur_ms ?? null,
    ttftMs: b.ttft_ms ?? null,
    reasonMs: b.reason_ms ?? null,
    retries: b.retries ?? 0,
    reset: b.reasoning_reset ?? false,
    tools: b.tool_calls.map((c) => c.name),
  }
}

const cache = new WeakMap<Node[], ReplyStat[]>()

/** The replies along `path` that made at least one model call. Memoized per
 *  path array, so every reply's card on a timeline shares one pass. */
export function repliesOf(path: Node[]): ReplyStat[] {
  const hit = cache.get(path)
  if (hit) return hit
  const out: ReplyStat[] = []
  let cur: ReplyStat | null = null
  let prompt: Node | null = null
  // End of the last step, and whether the reply had finished there (a call
  // with no tool calls): a gap after that is waiting, not work.
  let prevEnd = 0
  let idle = false
  // Failures recorded before the reply's first call (its first try failed).
  let failed = 0

  for (const n of path) {
    const b = n.body
    if (b.type === 'user' || compactionOf(n)) {
      cur = null
      prompt = b.type === 'user' ? n : null
      idle = false
      failed = 0
      continue
    }
    // A plugin invoke runs inside its enclosing call's duration.
    if (b.type === 'tool_result' && b.invoke) continue
    if (b.type === 'note') {
      // The branch stopped there; the retry starts whenever the user asks.
      if (b.content.data?.tag === TURN_ERROR_TAG) {
        if (cur) cur.failures++
        else failed++
        idle = true
      }
      continue
    }
    const dur = n.dur_ms ?? 0
    if (!cur) {
      if (b.type !== 'assistant') continue
      cur = {
        index: out.length + 1,
        id: n.id,
        prompt,
        calls: [],
        modelMs: 0,
        toolMs: 0,
        otherMs: 0,
        failures: failed,
      }
      out.push(cur)
      // The reply's time starts at its first call: no node records when the
      // turn began, and a regenerated reply starts long after its prompt.
      prevEnd = n.at - dur
    }
    const gap = Math.max(0, n.at - dur - prevEnd)
    if (!idle) cur.otherMs += gap
    prevEnd = Math.max(prevEnd, n.at)
    if (b.type === 'assistant') {
      cur.calls.push(callOf(n, cur.index))
      cur.modelMs += dur
      idle = b.tool_calls.length === 0
    } else {
      cur.toolMs += dur
      idle = false
    }
  }
  cache.set(path, out)
  return out
}

/** The sums over `replies`. */
export function summarize(replies: ReplyStat[]): Summary {
  const s: Summary = {
    replies: replies.length,
    calls: 0,
    activeMs: 0,
    waitMs: 0,
    genMs: 0,
    toolMs: 0,
    otherMs: 0,
    input: 0,
    cached: 0,
    written: 0,
    output: 0,
    hitRate: null,
    avgTtftMs: null,
    maxTtftMs: null,
    rate: null,
    retries: 0,
    failures: 0,
    resets: 0,
    models: [],
    lastInput: 0,
  }
  const models = new Map<string, number>()
  let ttftSum = 0
  let ttftN = 0
  // Generation time: after the first token where it is known, else the call.
  let genOut = 0
  let genMs = 0
  let modelMs = 0
  for (const r of replies) {
    modelMs += r.modelMs
    s.toolMs += r.toolMs
    s.otherMs += r.otherMs
    s.failures += r.failures
    for (const c of r.calls) {
      s.calls++
      s.input += c.input
      s.cached += c.cached
      s.written += c.written
      s.output += c.output
      s.retries += c.retries
      if (c.reset) s.resets++
      models.set(c.model, (models.get(c.model) ?? 0) + 1)
      if (c.ttftMs !== null) {
        ttftSum += c.ttftMs
        ttftN++
        s.maxTtftMs = Math.max(s.maxTtftMs ?? 0, c.ttftMs)
      }
      if (c.durMs !== null) {
        const wait = Math.min(c.ttftMs ?? 0, c.durMs)
        s.waitMs += wait
        const ms = c.durMs - wait
        if (ms > 0) {
          genOut += c.output
          genMs += ms
        }
      }
      s.lastInput = c.input
    }
  }
  s.genMs = Math.max(0, modelMs - s.waitMs)
  s.activeMs = modelMs + s.toolMs + s.otherMs
  s.hitRate = s.input > 0 ? s.cached / s.input : null
  s.avgTtftMs = ttftN > 0 ? ttftSum / ttftN : null
  s.rate = genMs > 0 && genOut > 0 ? (genOut * 1000) / genMs : null
  s.models = [...models].map(([ref, calls]) => ({ ref, calls })).sort((a, b) => b.calls - a.calls)
  return s
}

/** Output tokens per second of one call, after its first token where known. */
export function callRate(c: CallStat): number | null {
  if (c.durMs === null || c.output <= 0) return null
  const ms = c.durMs - (c.ttftMs ?? 0)
  return ms > 0 ? (c.output * 1000) / ms : null
}
