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

import { computed, reactive, ref, watch, type ComputedRef, type Ref } from 'vue'
import { rumu } from '../api'
import { t } from '../../i18n'
import type { Branch, BranchId, Node, Quote } from '../types'
import { state } from './state'
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
}

/** The side questions of the conversation `nodes` of branch `from`; it
 *  starts clean on another branch (another session, another sub-agent). */
export function makeAside(from: ComputedRef<BranchId | null>, nodes: ComputedRef<Node[]>): Aside {
  const a: Aside = {
    from,
    card: reactive({ open: false, folded: false, branch: null, quote: null }),
    selection: ref(null),
    quotes: ref([]),
    focus: ref(0),
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
  return a
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

/** Ask in the open card: a follow-up on its question, or a new one. */
export async function askSide(a: Aside, text: string): Promise<boolean> {
  try {
    if (a.card.branch) {
      await rumu.request('prompt', { branch: a.card.branch, text })
      return true
    }
    if (!a.from.value) return false
    const res = await rumu.request<{ branch: Branch }>('branch.side', {
      branch: a.from.value,
      text,
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
