// Card registry: identifying knowledge cards everywhere they appear.
//
// A card surfaces in four shapes:
//   1. Note { content.data = memory.import {id, rev} }              → context pill chip
//   2. Note { content.data = memory.card_update {id, rev} }         → "updated" pill
//   3. ToolResult of a card read/write (read/write/edit on
//      `@rumu:/cards/<id>`) + a card_use note → tool row chip
//   4. the Context panel (card.list + the uses derived from the path)
// Cards are identified by their plain path id everywhere (`@rumu:/cards/<id>`
// is the display form). This module maps all shapes to the same Card record.
// It mirrors the derivation in crates/rumu-cards/src/usage.rs.

import { computed } from 'vue'
import { parse as parseYaml } from 'yaml'
import type { Card, CardImport, CardUpdate, CardUse as CardUseRecord, Content, Node } from './types'
import { callsOnPath, pathNodes, state } from './store'

/** The payload tag/version decode for one Content, or null. */
function payload<T>(content: Content | null | undefined, tag: string, v: number): T | null {
  const d = content?.data
  if (!d || d.tag !== tag || d.v !== v) return null
  return d.data as T
}

/** An auto-import note's payload, if it is one. */
export function importOf(content: Content | null | undefined): CardImport | null {
  return payload<CardImport>(content, 'memory.import', 1)
}

/** A card update notice's payload, if it is one. */
export function updateOf(content: Content | null | undefined): CardUpdate | null {
  return payload<CardUpdate>(content, 'memory.card_update', 1)
}

/** The `card_use` record carried by an empty-text note. */
export function cardUseOf(content: Content | null | undefined): CardUseRecord | null {
  return payload<CardUseRecord>(content, 'card_use', 1)
}

/** The `workspace.use` marker carried by an empty-text note. */
export function workspaceUseOf(content: Content | null | undefined): { name: string } | null {
  return payload<{ name: string }>(content, 'workspace.use', 1)
}

/** The `workspace.agents` payload carried by an agents note. */
export function agentsOf(content: Content | null | undefined): import('./types').AgentsNote | null {
  return payload(content, 'workspace.agents', 1)
}

/** The `kernel.revert` payload carried by a revert lesson note. */
export function revertOf(content: Content | null | undefined): { lesson: string } | null {
  return payload<{ lesson: string }>(content, 'kernel.revert', 1)
}

/** The current render of a card (`# {title||id}\n{body}`). */
export function renderCard(card: Pick<Card, 'id' | 'title' | 'body'>): string {
  return `# ${card.title || card.id}\n${card.body}`
}

/** A card's editable view split into the fields its reader shows. */
export interface CardView {
  title: string | null
  description: string | null
  keywords: string[]
  meta: Record<string, unknown>
  body: string
}

/** Split leading `---\n…\n---\n` YAML frontmatter off a card view. Mirrors
 * `split_frontmatter` in crates/rumu-cards/src/card.rs. */
function splitFrontmatter(text: string): { yaml: string; body: string } | null {
  if (!text.startsWith('---\n')) return null
  const rest = text.slice(4)
  let start = 0
  while (start <= rest.length) {
    const end = rest.indexOf('\n', start)
    const line = end < 0 ? rest.slice(start) : rest.slice(start, end)
    if (line.trimEnd() === '---') {
      return { yaml: rest.slice(0, start), body: end < 0 ? '' : rest.slice(end + 1) }
    }
    if (end < 0) break
    start = end + 1
  }
  return null
}

/** Frontmatter `keywords:` — an array, or one comma-separated string. */
function keywordsOf(value: unknown): string[] {
  const list = typeof value === 'string' ? value.split(/[,，]/) : Array.isArray(value) ? value : []
  return list
    .filter((k): k is string => typeof k === 'string')
    .map((k) => k.trim())
    .filter(Boolean)
}

/**
 * Parse one recorded card snapshot. Tool read/write snapshots are the
 * editable view (YAML frontmatter + body); an auto-import snapshot is the
 * rendered `# Title\nbody` form (`rendered`).
 */
export function parseCardView(text: string, rendered = false): CardView {
  const empty: CardView = { title: null, description: null, keywords: [], meta: {}, body: text }
  const split = splitFrontmatter(text)
  let meta: Record<string, unknown> = {}
  let body = text
  if (split) {
    body = split.body
    try {
      const parsed: unknown = parseYaml(split.yaml)
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        meta = parsed as Record<string, unknown>
      }
    } catch {
      return empty // a malformed historical view: show it as written
    }
  }
  let title = typeof meta.title === 'string' ? meta.title : null
  if (rendered) {
    const nl = body.indexOf('\n')
    const heading = /^#\s+(.+?)\s*$/.exec(nl < 0 ? body : body.slice(0, nl))
    if (heading) {
      title ??= heading[1]!
      body = nl < 0 ? '' : body.slice(nl + 1)
    }
  }
  return {
    title,
    description: typeof meta.description === 'string' ? meta.description : null,
    keywords: keywordsOf(meta.keywords),
    meta,
    body,
  }
}

/** Does a use still match the current card content? */
export function useIsCurrent(card: Card, use: CardUse): boolean {
  return use.rev === card.revision
}

/** card id → record from card.list. */
export const cardById = computed(() => {
  const m = new Map<string, Card>()
  for (const c of state.cards) m.set(c.id, c)
  return m
})

export type UseKind = 'import' | 'read' | 'write'

export interface CardUse {
  kind: UseKind
  rev: string | null
  node: Node
  /** The exact text the model saw (note text / tool result / written body). */
  snapshot: string
  callId: string | null
}

/** Every recorded use of every card on the current path, in path order. */
export const cardUsesOnPath = computed(() => {
  const okCalls = new Set<string>()
  for (const n of pathNodes.value) {
    if (n.body.type === 'tool_result' && !n.body.is_error) okCalls.add(n.body.call_id)
  }
  const resultText = new Map<string, string>()
  for (const n of pathNodes.value) {
    if (n.body.type === 'tool_result' && !n.body.is_error) {
      resultText.set(n.body.call_id, n.body.content.text)
    }
  }
  const calls = callsOnPath.value

  const uses = new Map<string, CardUse[]>()
  const push = (id: string, u: CardUse): void => {
    if (!uses.has(id)) uses.set(id, [])
    uses.get(id)!.push(u)
  }

  for (const n of pathNodes.value) {
    const b = n.body
    if (b.type !== 'note') continue
    const imp = importOf(b.content)
    if (imp) {
      push(imp.id, {
        kind: 'import',
        rev: imp.rev,
        node: n,
        snapshot: b.content.text,
        callId: null,
      })
      continue
    }
    const u = cardUseOf(b.content)
    if (!u) continue
    const callId = u.call_id ?? null
    if (!u.id || !u.rev) continue
    // Abort-path guard: only count records whose call succeeded.
    if (callId && !okCalls.has(callId)) continue
    const op = u.op === 'read' ? 'read' : 'write'
    // Write snapshots come from the call args (`content`, the editable view).
    const snapshot =
      op === 'write'
        ? String((calls.get(callId ?? '')?.call.args as { content?: unknown } | undefined)?.content ?? '')
        : (resultText.get(callId ?? '') ?? '')
    push(u.id, { kind: op, rev: u.rev, node: n, snapshot, callId })
  }
  return uses
})

/** Card id → update notes on this path (`memory.card_update`), in order. */
export const updateNotesByCard = computed(() => {
  const m = new Map<string, Node[]>()
  for (const n of pathNodes.value) {
    if (n.body.type !== 'note') continue
    const u = updateOf(n.body.content)
    if (!u) continue
    if (!m.has(u.id)) m.set(u.id, [])
    m.get(u.id)!.push(n)
  }
  return m
})

/** Fallback title parsed from an injected note text ('# Title\nbody'). */
export function noteTitleFromText(text: string): string {
  const line = text.split('\n')[0] ?? ''
  return line.replace(/^#\s*/, '').trim()
}

/** Look up a card for an import payload; null if none. */
export function cardForImport(imp: CardImport | null): Card | null {
  return imp ? (cardById.value.get(imp.id) ?? null) : null
}
