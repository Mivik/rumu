// Public facade for the app state. The implementation is split by concern in
// ./state/; components keep importing from this module.
//
//   state.ts    the reactive slices (data only)
//   derived.ts  read-only computed views
//   timeline.ts chat grouping (pure derivation)
//   ui.ts       toasts / drawer / focus
//   tree.ts     session tree: load, switch, fork, regenerate
//   sessions.ts session lifecycle and prompting
//   pushes.ts   WebSocket pushes + boot

export { state } from './state/state'
export type { DeltaBuffer, HookTraceEntry, Toast, TurnCache } from './state/state'

export {
  activeDelta,
  branchTasks,
  branchTree,
  cacheTotal,
  cacheTurns,
  callsOnPath,
  connected,
  currentBranch,
  hookTraceForSession,
  isBusy,
  pathNodes,
  queuedPrompts,
  runningTasks,
  sortedSessions,
} from './state/derived'
export type { BranchTreeNode } from './state/derived'

export { timeline } from './state/timeline'
export type {
  ActivityStep,
  ActivityThought,
  ActivityTools,
  GroupActivityItem,
  GroupContextItem,
  GroupItem,
  GroupTextItem,
  TimelineContextEntry,
  TimelineEntry,
  TimelineGroup,
  TimelineUserEntry,
  ToolRowInfo,
} from './state/timeline'

export { closeDrawer, dismissToast, focusNode, openDrawer } from './state/ui'

export { findAgentChild, forkFrom, regenerateFrom, selectBranch } from './state/tree'
export type { OpenHistory } from './state/sessions'
export {
  closeSession,
  editPrompt,
  goHome,
  killTask,
  newSession,
  openSession,
  openTaskDetail,
  refreshTasks,
  sendPrompt,
  stopTurn,
} from './state/sessions'

export { startApp } from './state/pushes'
