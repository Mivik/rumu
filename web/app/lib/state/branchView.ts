// Which branch a conversation shows. The chat shows the session's main line;
// the sub-agent panel beside it shows a sub-agent (`state.agentView`). The
// conversation's components — timeline, composer, tasks — read their branch
// through `useBranchView`, so one set of them serves both.

import { computed, inject, provide, shallowRef, watch, type ComputedRef, type InjectionKey } from 'vue'
import type { Branch, BranchId, ModelUse, Node, TaskInfo } from '../types'
import type { DeltaBuffer } from './state'
import { state } from './state'
import { contextUseOf, type ContextUse } from './context'
import { callsOf, nestedRunningOf, owedOf, pathOf, runningOf, type OwedReply } from './derived'
import { branchPick, currentPick, pick, pickFor } from './models'
import { makeAside, type Aside } from './side'
import { buildTimeline, shownOf, type TimelineEntry } from './timeline'

export interface BranchView {
  branch: ComputedRef<BranchId | null>
  /** The branch's whole path, root → head. */
  nodes: ComputedRef<Node[]>
  /** What the conversation shows of it, grouped. */
  timeline: ComputedRef<TimelineEntry[]>
  delta: ComputedRef<DeltaBuffer | null>
  busy: ComputedRef<boolean>
  /** The reply it still owes after a failure. */
  owed: ComputedRef<OwedReply | null>
  /** Steering prompts waiting to be written. */
  queued: ComputedRef<{ id: string; text: string }[]>
  /** Its still-running tasks, oldest first. */
  tasks: ComputedRef<TaskInfo[]>
  /** Tasks its sub-agents (at any depth) still run. */
  nestedTasks: ComputedRef<number>
  /** The model the next message runs on (`null`: the assistant's), and
   *  picking another. */
  model: ComputedRef<ModelUse | null>
  pick: (use: ModelUse | null) => void
  /** How much of that model's window the next request fills. */
  context: ComputedRef<ContextUse | null>
  /** Its side questions, selection and quotes. */
  aside: Aside
  /** A sub-agent's or side question's branch: it is not the main line, so
   *  its turns offer no edit, regenerate or fork, and its composer has no
   *  workspace or decisions (those stay with the main line). */
  sub: boolean
  /** A side question's view: the aside of the conversation it was asked
   *  off, which its composer asks in. */
  side: Aside | null
  /** Names the view on the page (`data-view` on what holds it; the main
   *  line's is the rest of the page). */
  key: string
  /** `el` lies in this view rather than another one on the page. */
  holds: (el: Element) => boolean
}

function branchView(
  id: () => BranchId | null,
  sub: boolean,
  key: string,
  side: Aside | null = null,
): BranchView {
  const branch = computed(id)
  const current = computed<Branch | null>(() =>
    branch.value ? (state.branches[branch.value] ?? null) : null,
  )
  const nodes = computed(() => pathOf(current.value))
  // the main line shown short of its end (`state.lineEnd`): nothing live
  // or owed shows at the end of what is shown
  const cut = computed(() => !sub && !!state.lineEnd)
  // the main line's pick is the session's (the landing page's before it);
  // a side question not asked yet runs on what it is asked off, or the pick
  // made for it in its card
  const fresh = shallowRef<{ use: ModelUse | null } | null>(null)
  if (side) watch(branch, () => (fresh.value = null))
  const model = computed(() =>
    side && !branch.value
      ? fresh.value
        ? fresh.value.use
        : side.model()
      : sub
        ? branchPick(branch.value, nodes.value)
        : currentPick(),
  )
  return {
    branch,
    nodes,
    timeline: computed(() =>
      buildTimeline(
        shownOf(current.value, nodes.value),
        callsOf(nodes.value),
        cut.value ? null : branch.value,
      ),
    ),
    delta: computed(() => (branch.value ? (state.deltas[branch.value] ?? null) : null)),
    busy: computed(() => !!branch.value && !!state.busy[branch.value]),
    owed: computed(() => (cut.value ? null : owedOf(branch.value, nodes.value))),
    queued: computed(() => (branch.value ? (state.queuedPrompts[branch.value] ?? []) : [])),
    tasks: computed(() => runningOf(branch.value)),
    nestedTasks: computed(() => nestedRunningOf(branch.value)),
    model,
    pick: !sub
      ? pick
      : (use) => (side && !branch.value ? (fresh.value = { use }) : pickFor(branch.value, nodes.value, use)),
    context: computed(() => contextUseOf(branch.value, nodes.value, model.value)),
    aside: makeAside(branch, nodes, () => model.value),
    sub,
    side,
    key,
    holds: (el) => (el.closest('[data-view]')?.getAttribute('data-view') ?? 'main') === key,
  }
}

const KEY: InjectionKey<BranchView> = Symbol('branch-view')

const mainView = branchView(() => state.mainBranch, false, 'main')

/** The sub-agent open beside the chat. */
export const subView = (): BranchView => branchView(() => state.agentView, true, 'agent')

/** The side question open in `from`'s card. */
export const sideView = (from: BranchView): BranchView =>
  branchView(() => from.aside.card.branch, true, `side:${from.key}`, from.aside)

export function provideBranchView(view: BranchView): void {
  provide(KEY, view)
}

export function useBranchView(): BranchView {
  return inject(KEY, mainView)
}

/** Open the sub-agent branch `id` beside the chat (it replaces an open
 *  artifact or task output); `null` closes it. */
export function openAgentView(id: BranchId | null): void {
  state.agentView = id
  state.panelBack = null
  if (!id) return
  state.artifact = null
  state.taskDetail = null
}
