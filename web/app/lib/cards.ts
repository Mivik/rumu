// Card registry: identifying knowledge cards everywhere they appear.
//
// A card surfaces in four shapes:
//   1. Note { content.data = memory.import {id, rev} }              → context pill chip
//   2. Note { content.data = memory.card_update {id, rev} }         → "updated" pill
//   3. ToolResult of a card read/write (read/write/edit on
//      `@rumu:/cards/<id>`) + a card.use note → tool row chip
//   4. the card window, which reads the live card (card.get) beside the uses
//      derived from the path
// A card is identified by its deck id and path; records carry both. This
// module maps all shapes to the same Card record. It mirrors the derivation in
// crates/rumu-cards/src/usage.rs.

import { computed } from 'vue'
import { parse as parseYaml, stringify as stringifyYaml } from 'yaml'
import type {
  Card,
  CardImport,
  CardUpdate,
  CardUse as CardUseRecord,
  Content,
  LibraryRef,
  Node,
} from './types'
import { rumu } from './api'
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

/** The `card.use` record carried by an empty-text note. */
export function cardUseOf(content: Content | null | undefined): CardUseRecord | null {
  return payload<CardUseRecord>(content, 'card.use', 1)
}

/** The `workspace.use` marker carried by an empty-text note. */
export function workspaceUseOf(content: Content | null | undefined): { name: string } | null {
  return payload<{ name: string }>(content, 'workspace.use', 1)
}

/** The `workspace.agents` payload carried by an agents note. */
export function agentsOf(content: Content | null | undefined): import('./types').AgentsNote | null {
  return payload(content, 'workspace.agents', 1)
}

/** The current render of a card (`# {id}\n{body}`). */
export function renderCard(card: Pick<Card, 'id' | 'body'>): string {
  return `# ${card.id}\n${card.body}`
}

/** A card's editable view split into the fields its reader shows. */
export interface CardView {
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
 * rendered `# id\nbody` form (`rendered`); its heading is dropped.
 */
export function parseCardView(text: string, rendered = false): CardView {
  const empty: CardView = { description: null, keywords: [], meta: {}, body: text }
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
  if (rendered) {
    const nl = body.indexOf('\n')
    if (/^#\s+.+$/.test(nl < 0 ? body : body.slice(0, nl))) body = nl < 0 ? '' : body.slice(nl + 1)
  }
  return {
    description: typeof meta.description === 'string' ? meta.description : null,
    keywords: keywordsOf(meta.keywords),
    meta,
    body,
  }
}

/** The fields the library form edits, split out of an editable view. */
export interface CardForm {
  description: string
  keywords: string[]
  pinned: boolean
  /** YAML for every other frontmatter key (`""` when none). */
  extra: string
  body: string
}

/** The frontmatter keys the library form edits itself. */
const FORM_KEYS = new Set(['description', 'keywords', 'pinned', 'link'])

/** The fields the library form edits, split out of an editable view. */
export function formOfView(view: string): CardForm {
  const split = splitFrontmatter(view)
  let meta: Record<string, unknown> = {}
  if (split) {
    try {
      const parsed: unknown = parseYaml(split.yaml)
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        meta = parsed as Record<string, unknown>
      }
    } catch {
      // A malformed view is shown as written; nothing can be split from it.
    }
  }
  const extra: Record<string, unknown> = {}
  for (const [key, value] of Object.entries(meta)) {
    if (!FORM_KEYS.has(key)) extra[key] = value
  }
  return {
    description: typeof meta.description === 'string' ? meta.description : '',
    keywords: keywordsOf(meta.keywords),
    pinned: meta.pinned === true,
    extra: Object.keys(extra).length ? stringifyYaml(extra, { indentSeq: false }).trimEnd() : '',
    body: split ? split.body : view,
  }
}

/** Inverse of `formOfView`: frontmatter (description, keywords, pinned, then
 *  the extra keys) + body. Empty values are omitted; no frontmatter block when
 *  nothing is left. Throws on invalid `extra` YAML. */
export function viewOfForm(form: CardForm): string {
  let extra: Record<string, unknown> = {}
  const text = form.extra.trim()
  if (text) {
    const parsed: unknown = parseYaml(text)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new Error('frontmatter must be a YAML mapping (key: value lines)')
    }
    extra = parsed as Record<string, unknown>
  }
  const meta: Record<string, unknown> = {}
  if (form.description.trim()) meta.description = form.description.trim()
  if (form.keywords.length) meta.keywords = [...form.keywords]
  if (form.pinned) meta.pinned = true
  for (const [key, value] of Object.entries(extra)) {
    if (!FORM_KEYS.has(key)) meta[key] = value
  }
  const front = Object.keys(meta).length
    ? `---\n${stringifyYaml(meta, { indentSeq: false }).trimEnd()}\n---\n`
    : ''
  return front + form.body
}

/** Does a use still match the current card content? */
export function useIsCurrent(card: Card, use: CardUse): boolean {
  return use.rev === card.revision
}

/** The current mount table as `{ point, deck }` (`/cards/self` → `self`, the
 * assistant's own deck), longest point first so a nested mount wins over its
 * parent. */
const cardMounts = computed(() => {
  const live = state.activeBranch && state.pathState
  const mounts = live
    ? state.pathState!.mounts
    : (state.assistants.find((a) => a.id === state.assistant)?.mounts ?? [])
  const out = new Map([['self', live ? state.pathState!.assistant : state.assistant]])
  for (const m of mounts) {
    const point = m.at.replace(/^\/?cards\/?/, '').replace(/\/+$/, '')
    if (point) out.set(point, m.deck)
  }
  return [...out].map(([point, deck]) => ({ point, deck })).sort((a, b) => b.point.length - a.point.length)
})

/** A mounted card id (`user/dev/x`) → its deck and deck-local path (`dev/x`);
 * a mount point itself maps to the deck root (`path: ''`). Null outside every
 * mount. */
export function mountedRef(id: string): LibraryRef | null {
  for (const { point, deck } of cardMounts.value) {
    if (id === point) return { deck, path: '' }
    if (id.startsWith(`${point}/`)) return { deck, path: id.slice(point.length + 1) }
  }
  return null
}

/** The mounted id for a deck-local path, under the deck's first mount point
 * (`self/dev/x`, `user/notes/y`); null when the deck is not mounted. */
export function mountedId(deck: string, path: string): string | null {
  const mount = cardMounts.value.find((m) => m.deck === deck)
  return mount ? `${mount.point}/${path}` : null
}

/** A card's identity key: the link target's `deck:path` for a link, else its
 * own `deck:path`. Usage payloads carry the identity. */
export function cardKey(card: Pick<Card, 'deck' | 'path' | 'link'>): string {
  return card.link ?? `${card.deck}:${card.path}`
}

/** How many cards the mounted decks hold (the sidebar's count). */
export const mountedCardCount = computed(() => {
  const mounted = new Set(cardMounts.value.map((m) => m.deck))
  return state.library.decks.reduce((sum, d) => sum + (mounted.has(d.id) ? d.cards : 0), 0)
})

/** The mounted card `id` (`self/dev/x`) as the open branch sees it (else the
 *  assistant in view); null when no card is there (a folder, or gone). */
export function fetchCard(id: string): Promise<Card | null> {
  const scope = state.activeBranch ? { branch: state.activeBranch } : { assistant: state.assistant }
  return rumu.request<Card | null>('card.get', { ...scope, id })
}

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
  // Successful call id → result text; invoke call id → its effective args.
  const resultText = new Map<string, string>()
  const invokeArgs = new Map<string, unknown>()
  for (const { body: b } of pathNodes.value) {
    if (b.type !== 'tool_result') continue
    if (!b.is_error) resultText.set(b.call_id, b.content.text)
    if (b.invoke) invokeArgs.set(b.call_id, b.invoke.args)
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
      push(`${imp.deck}:${imp.path}`, {
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
    if (!u.deck || !u.path || !u.rev) continue
    // Abort-path guard: only count records whose call succeeded — an invoke
    // has its own result node, so the same rule covers it.
    if (callId && !resultText.has(callId)) continue
    const op = u.op === 'read' ? 'read' : 'write'
    // Write snapshots come from the call args (`content`, the editable view).
    const args = calls.get(callId ?? '')?.call.args ?? invokeArgs.get(callId ?? '')
    const snapshot =
      op === 'write'
        ? String((args as { content?: unknown } | undefined)?.content ?? '')
        : (resultText.get(callId ?? '') ?? '')
    push(`${u.deck}:${u.path}`, { kind: op, rev: u.rev, node: n, snapshot, callId })
  }
  return uses
})

/** Card identity → update notes on this path (`memory.card_update`), in order. */
export const updateNotesByCard = computed(() => {
  const m = new Map<string, Node[]>()
  for (const n of pathNodes.value) {
    if (n.body.type !== 'note') continue
    const u = updateOf(n.body.content)
    if (!u) continue
    const key = `${u.deck}:${u.path}`
    if (!m.has(key)) m.set(key, [])
    m.get(key)!.push(n)
  }
  return m
})
