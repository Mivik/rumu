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
export type { DeltaBuffer, HookTraceEntry, RetryState, Toast, TurnCache } from './state/state'

export {
  activeDelta,
  branchTasks,
  cacheTotal,
  cacheTurns,
  callsOnPath,
  connected,
  hookTraceForSession,
  isBusy,
  owedReply,
  pathNodes,
  queuedPrompts,
  runningTasks,
  siblingAlternatives,
  sortedSessions,
} from './state/derived'
export type { OwedReply, SiblingAlternative } from './state/derived'

export { pendingCalls, timeline } from './state/timeline'
export type {
  ActivityCatalog,
  ActivityRecall,
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

export { closeDrawer, dismissToast, openDrawer, openLightbox, closeLightbox } from './state/ui'

export { findAgentChild, forkAt, regenerateFrom, retryBranch, selectBranch } from './state/tree'
export type { OpenHistory, PromptAttachment } from './state/sessions'

export {
  cardHistory,
  cardVersion,
  closeCard,
  closeLibrary,
  addFolder,
  createDeck,
  deleteCard,
  deleteDeck,
  deleteFolder,
  loadCard,
  moveEntry,
  newCard,
  openCard,
  openLibrary,
  openLibraryAtDeck,
  refreshLibrary,
  renameDeck,
  saveCard,
  searchLibrary,
} from './state/library'
export type { SaveCardRequest } from './state/library'
export {
  answerDecision,
  closeSession,
  deleteSession,
  editPrompt,
  forkSession,
  forgetSession,
  goHome,
  killTask,
  newSession,
  openSession,
  openTaskDetail,
  defaultAssistant,
  refreshAssistants,
  refreshTasks,
  sendPrompt,
  stopTurn,
  viewAssistant,
} from './state/sessions'

export { startApp } from './state/pushes'
