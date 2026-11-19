// Which branch the chat shows: the session's main line, or a sub-agent
// opened from it. Opening one is client-side only — the session's main
// line (`state.session.main_branch`) stays put, and the chat (timeline,
// composer, stop, tasks) simply works on the sub-agent's branch, as on any.

import { computed, type ComputedRef } from 'vue'
import type { BranchId, Node } from '../types'
import type { DeltaBuffer } from './state'
import { state } from './state'
import { activeDelta, currentBranch, isBusy, pathNodes } from './derived'
import { loadPath } from './tree'

export interface BranchView {
  branch: ComputedRef<BranchId | null>
  /** The branch's whole path, root → head. */
  nodes: ComputedRef<Node[]>
  delta: ComputedRef<DeltaBuffer | null>
  busy: ComputedRef<boolean>
  /** A sub-agent's branch: it belongs to the call that started it, so its
   *  turns offer no edit, regenerate or fork. */
  readonly sub: boolean
}

const mainView: BranchView = {
  branch: computed(() => state.activeBranch),
  nodes: pathNodes,
  delta: activeDelta,
  busy: isBusy,
  get sub() {
    return currentBranch.value?.kind.type === 'agent'
  },
}

export function useBranchView(): BranchView {
  return mainView
}

/** Show the sub-agent branch `id` in the chat; `null` goes back to the
 *  session's main line. */
export function openAgentView(id: BranchId | null): void {
  const target = id ?? state.session?.main_branch ?? null
  if (!target || target === state.activeBranch) return
  state.activeBranch = target
  state.pathState = null
  void loadPath(target)
}
