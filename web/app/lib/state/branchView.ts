// Which branch a timeline renders. The chat shows the active branch; the
// sub-agent panel shows a child branch beside it without switching to it.
// The timeline's components read their branch through `useBranchView`, so
// one set of components serves both.

import { computed, inject, provide, type ComputedRef, type InjectionKey } from 'vue'
import type { BranchId, Node } from '../types'
import type { DeltaBuffer } from './state'
import { state } from './state'
import { activeDelta, isBusy, pathNodes } from './derived'

export interface BranchView {
  branch: ComputedRef<BranchId | null>
  /** The branch's whole path, root → head. */
  nodes: ComputedRef<Node[]>
  delta: ComputedRef<DeltaBuffer | null>
  busy: ComputedRef<boolean>
  /** A sub-agent's transcript: read-only, so no edit, regenerate or fork. */
  sub: boolean
}

const KEY: InjectionKey<BranchView> = Symbol('branch-view')

const mainView: BranchView = {
  branch: computed(() => state.activeBranch),
  nodes: pathNodes,
  delta: activeDelta,
  busy: isBusy,
  sub: false,
}

export function provideBranchView(view: BranchView): void {
  provide(KEY, view)
}

export function useBranchView(): BranchView {
  return inject(KEY, mainView)
}

/** Open the sub-agent branch `id` beside the chat (it replaces an open
 *  artifact or task output). */
export function openAgentView(id: BranchId | null): void {
  state.agentView = id
  if (!id) return
  state.artifact = null
  state.taskDetail = null
}
