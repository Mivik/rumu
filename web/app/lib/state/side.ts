// Side questions (kernel `branch.side`): asides off a conversation — the
// main line or a sub-agent — that it never sees. One card at the
// conversation's top right shows them (SidePop); it opens only when asked —
// `/btw`, or a passage selected in the conversation — and leaves nothing
// behind but a dotted underline on the passages it was asked about.
//
// The same selection also quotes into the composer: picked passages ride
// along as markdown quotes ahead of the message.
//
// Each conversation shown (BranchView) has its own: the main line's and the
// sub-agent panel's card, selection and quotes are apart.

import { computed, onScopeDispose, reactive, ref, watch, type ComputedRef, type Ref } from 'vue'
import { rumu } from '../api'
import { t } from '../../i18n'
import type { Branch, BranchId, ModelUse, Node, Quote } from '../types'
import { state } from './state'
import { models } from './models'
import { toast } from './ui'

export interface Aside {
  /** The branch asked off. */
  from: ComputedRef<BranchId | null>
  card: {
    open: boolean
    /** Folded down to its title row. */
    folded: boolean
    /** The side question shown; `null`: the list, or a new question. */
    branch: BranchId | null
    /** A new question's passage, until it is sent. */
    quote: Quote | null
  }
  /** The passage selected in the conversation, while there is one. */
  selection: Ref<Quote | null>
  /** Passages quoted into the composer, sent ahead of the message. */
  quotes: Ref<Quote[]>
  /** Its side questions off the conversation on screen, newest first. */
  branches: ComputedRef<Branch[]>
  /** Bumped to put the caret in this conversation's composer (a quote
   *  just went in). */
  focus: Ref<number>
  /** The draft its composer writes on, once one is mounted. */
  draft: () => { text: string } | null
  /** The model the conversation's next message runs on: a new side
   *  question's, unless one is picked in its card. */
  model: () => ModelUse | null
}

/** The side questions of the conversation `nodes` of branch `from`; it
 *  starts clean on another branch (another session, another sub-agent). */
export function makeAside(
  from: ComputedRef<BranchId | null>,
  nodes: ComputedRef<Node[]>,
  model: () => ModelUse | null = () => null,
): Aside {
  const a: Aside = {
    from,
    card: reactive({ open: false, folded: false, branch: null, quote: null }),
    selection: ref(null),
    quotes: ref([]),
    focus: ref(0),
    draft: () => null,
    model,
    branches: computed(() => {
      const onPath = new Set(nodes.value.map((n) => n.id))
      return Object.values(state.branches)
        .filter(
          (b) =>
            b.kind.type === 'side' &&
            b.kind.parent_branch === from.value &&
            !!b.forked_from &&
            onPath.has(b.forked_from),
        )
        .sort((x, y) => y.created_at - x.created_at)
    }),
  }
  watch(from, () => {
    closeSide(a)
    a.quotes.value = []
    a.selection.value = null
  })
  live.add(a)
  onScopeDispose(() => live.delete(a))
  return a
}

/** The asides of the conversations on screen. */
const live = new Set<Aside>()

/** The aside of the conversation on screen showing branch `from`. */
export function asideOf(from: BranchId): Aside | null {
  for (const a of live) if (a.from.value === from) return a
  return null
}

/** Open the card on `branch`, or on a new question about `quote` (`null`
 *  and no quote: the list, when there is one). */
export function openSide(a: Aside, branch: BranchId | null, quote: Quote | null = null): void {
  a.card.open = true
  a.card.folded = false
  a.card.branch = branch
  a.card.quote = branch ? null : quote
}

export function closeSide(a: Aside): void {
  a.card.open = false
  a.card.branch = null
  a.card.quote = null
}

/** What a question carries besides its text, as `prompt` takes it: stored
 *  attachments, and the model it runs on where one was picked. */
export interface AskExtras {
  attachments?: { hash: string; name?: string }[]
  model?: ModelUse | null
}

/** Ask in the open card: a follow-up on its question, or a new one. */
export async function askSide(a: Aside, text: string, extras: AskExtras = {}): Promise<boolean> {
  const params: Record<string, unknown> = { text }
  if (extras.attachments?.length) params.attachments = extras.attachments
  if (extras.model !== undefined) params.model = extras.model
  try {
    if (a.card.branch) {
      await rumu.request('prompt', { branch: a.card.branch, ...params })
      delete models.pending[a.card.branch]
      return true
    }
    if (!a.from.value) return false
    const res = await rumu.request<{ branch: Branch }>('branch.side', {
      branch: a.from.value,
      ...params,
      quote: a.card.quote,
    })
    state.branches[res.branch.id] ??= res.branch
    a.card.branch = res.branch.id
    a.card.quote = null
    return true
  } catch (err) {
    toast('error', t('chat.side.failed'), (err as Error).message)
    return false
  }
}

/** Quote a passage into the composer (once), the caret going there. */
export function addQuote(a: Aside, q: Quote): void {
  if (!a.quotes.value.some((x) => x.node === q.node && x.text === q.text)) a.quotes.value.push(q)
  a.focus.value++
}

/** Put a side question's address into the conversation's composer: the
 *  main line's model can read it, and a click on it opens the question
 *  again. What a side question turned up joins the conversation only this
 *  way, or as a quote. */
export function linkSide(a: Aside, branch: BranchId): void {
  const at = `@rumu:/sessions/${state.sessionId}/branches/${branch.slice(0, 8)}`
  const draft = a.draft()
  if (!draft) return
  draft.text = draft.text.includes(at)
    ? draft.text
    : [draft.text.trimEnd(), at].filter(Boolean).join(' ') + ' '
  a.focus.value++
}

/** A passage as the markdown quote it is sent as. */
export function quoteBlock(q: Quote): string {
  return q.text
    .split('\n')
    .map((line) => `> ${line}`.trimEnd())
    .join('\n')
}

// ---- the passages asked about, underlined ----

const painted = new Map<object, Range[]>()

/** Underline `ranges` for the conversation `key` (each paints its own; the
 *  page holds one highlight for all of them). */
export function paintQuotes(key: object, ranges: Range[] | null): void {
  if (!('highlights' in CSS)) return
  if (ranges?.length) painted.set(key, ranges)
  else painted.delete(key)
  CSS.highlights.set('side-quote', new Highlight(...[...painted.values()].flat()))
}
