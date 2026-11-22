// Which branch a conversation shows. The chat shows the session's main line;
// the sub-agent panel beside it shows a sub-agent (`state.agentView`). The
// conversation's components — timeline, composer, tasks — read their branch
// through `useBranchView`, so one set of them serves both.

import { computed, inject, provide, type ComputedRef, type InjectionKey } from 'vue'
import type { Branch, BranchId, Node, TaskInfo } from '../types'
import type { DeltaBuffer } from './state'
import { state } from './state'
import { callsOf, nestedRunningOf, owedOf, pathOf, runningOf, type OwedReply } from './derived'
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
  /** A sub-agent's branch: it belongs to the call that started it, so its
   *  turns offer no edit, regenerate or fork, and the composer only talks to
   *  it (no model, quotes or side questions). */
  sub: boolean
}

function branchView(id: () => BranchId | null, sub: boolean): BranchView {
  const branch = computed(id)
  const current = computed<Branch | null>(() =>
    branch.value ? (state.branches[branch.value] ?? null) : null,
  )
  const nodes = computed(() => pathOf(current.value))
  // the main line shown short of its end (`state.lineEnd`): nothing live
  // or owed shows at the end of what is shown
  const cut = computed(() => !sub && !!state.lineEnd)
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
    sub,
  }
}

const KEY: InjectionKey<BranchView> = Symbol('branch-view')

const mainView = branchView(() => state.mainBranch, false)

/** The sub-agent open beside the chat. */
export const subView = (): BranchView => branchView(() => state.agentView, true)

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
