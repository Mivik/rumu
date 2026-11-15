// Side questions (kernel `branch.side`): asides off the main line that the
// main line never sees. One card at the sheet's top right shows them
// (SidePop); it opens only when asked — `/btw`, or a passage selected in the
// conversation — and leaves nothing behind but a dotted underline on the
// passages it was asked about.
//
// The same selection also quotes into the composer: picked passages ride
// along as markdown quotes ahead of the message.

import { computed, reactive, ref, watch } from 'vue'
import { rumu } from '../api'
import { t } from '../../i18n'
import type { Branch, BranchId, Quote } from '../types'
import { pathNodes } from './derived'
import { state } from './state'
import { toast } from './ui'

export const side = reactive({
  open: false,
  /** Folded down to its title row. */
  folded: false,
  /** The side question shown; `null`: the list, or a new question. */
  branch: null as BranchId | null,
  /** A new question's passage, until it is sent. */
  quote: null as Quote | null,
})

/** The passage selected in the conversation, while there is one. */
export const selection = ref<Quote | null>(null)

/** Passages quoted into the composer, sent ahead of the message. */
export const quotes = ref<Quote[]>([])

/** The open session's side questions off the conversation on screen,
 *  newest first. */
export const sideBranches = computed<Branch[]>(() => {
  const onPath = new Set(pathNodes.value.map((n) => n.id))
  return Object.values(state.branches)
    .filter((b) => b.kind.type === 'side' && !!b.forked_from && onPath.has(b.forked_from))
    .sort((a, b) => b.created_at - a.created_at)
})

/** Open the card on `branch`, or on a new question about `quote` (`null`
 *  and no quote: the list, when there is one). */
export function openSide(branch: BranchId | null, quote: Quote | null = null): void {
  side.open = true
  side.folded = false
  side.branch = branch
  side.quote = branch ? null : quote
}

export function closeSide(): void {
  side.open = false
  side.branch = null
  side.quote = null
}

// Another session starts clean: its card closed, nothing quoted.
watch(
  () => state.sessionId,
  () => {
    closeSide()
    quotes.value = []
    selection.value = null
  },
)

/** Ask in the open card: a follow-up on its question, or a new one. */
export async function askSide(text: string): Promise<boolean> {
  try {
    if (side.branch) {
      await rumu.request('prompt', { branch: side.branch, text })
      return true
    }
    if (!state.activeBranch) return false
    const res = await rumu.request<{ branch: Branch }>('branch.side', {
      branch: state.activeBranch,
      text,
      quote: side.quote,
    })
    state.branches[res.branch.id] ??= res.branch
    side.branch = res.branch.id
    side.quote = null
    return true
  } catch (err) {
    toast('error', t('chat.side.failed'), (err as Error).message)
    return false
  }
}

/** Quote a passage into the composer (once). */
export function addQuote(q: Quote): void {
  if (!quotes.value.some((x) => x.node === q.node && x.text === q.text)) quotes.value.push(q)
}

/** A passage as the markdown quote it is sent as. */
export function quoteBlock(q: Quote): string {
  return q.text
    .split('\n')
    .map((line) => `> ${line}`.trimEnd())
    .join('\n')
}
