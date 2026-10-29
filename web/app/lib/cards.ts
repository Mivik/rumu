// Card registry: identifying knowledge cards everywhere they appear.
//
// A card surfaces in four shapes:
//   1. Note { content.data = memory.import {id, rev} }              → context pill chip
//   2. Note { content.data = memory.card_update {id, rev} }         → "updated" pill
//   3. ToolResult of a card read/write (read/write/edit on
//      `@rumu:/card/<id>`) + a card_use note → tool row chip
//   4. the Context panel (card.list + the uses derived from the path)
// Cards are identified by their plain path id everywhere (`@rumu:/card/<id>`
// is the display form). This module maps all shapes to the same Card record.
// It mirrors the derivation in crates/rumu-cards/src/usage.rs.

import { computed } from 'vue'
import type { Card, CardImport, CardUpdate, CardUse as CardUseRecord, Content, Node } from './types'
import { callsOnPath, hookTraceForSession, pathNodes, state } from './store'

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

/** Card ids used on the current path (imported, read or written). */
export const activeCardIds = computed(() => new Set(cardUsesOnPath.value.keys()))

/** The latest use of each active card. */
export const latestUseByCard = computed(() => {
  const m = new Map<string, CardUse>()
  for (const [id, list] of cardUsesOnPath.value) m.set(id, list[list.length - 1]!)
  return m
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

/**
 * Which hook injected a note, from the live hook trace
 * (e.g. 'memory · recall'). Live sessions only — null when unknown.
 * Matches the effect's structured payload when present, else its text.
 */
export function importedBy(content: Content | null | undefined): string | null {
  if (!content) return null
  const imp = importOf(content)
  const trace = hookTraceForSession.value
  for (let i = trace.length - 1; i >= 0; i--) {
    const e = trace[i]
    if (
      e.effects.some((a) => {
        if (imp) {
          const ai = importOf(a)
          return ai?.id === imp.id && ai.rev === imp.rev
        }
        return a.text === content.text && a.data?.tag === content.data?.tag
      })
    ) {
      return `${e.plugin} · ${e.hook_id}`
    }
  }
  return null
}
