// Public facade for the app state. The implementation is split by concern in
// ./state/; components keep importing from this module.
//
//   state.ts    the reactive slices (data only)
//   derived.ts  read-only computed views
//   timeline.ts chat grouping (pure derivation)
//   ui.ts       toasts / drawer / focus
//   tree.ts     session tree: load, switch, fork
//   sessions.ts session lifecycle, prompting, regenerating and retrying
//   pushes.ts   WebSocket pushes + boot

export { state } from './state/state'
export type { DeltaBuffer, HookTraceEntry, RetryState, Toast } from './state/state'
export type { CardWindow } from './state/state'

export {
  activeDelta,
  branchTasks,
  callsOf,
  callsOnPath,
  connected,
  hookTraceForSession,
  isBusy,
  isUnseen,
  owedReply,
  pathNodes,
  pathOf,
  queuedPrompts,
  revertibleFrom,
  runningTasks,
  siblingAlternatives,
  sortedSessions,
} from './state/derived'
export type { OwedReply, SiblingAlternative } from './state/derived'

export { buildTimeline, compactionOf, timeline } from './state/timeline'
export { callRate, repliesOf, summarize } from './state/stats'
export type { CallStat, ReplyStat, Summary } from './state/stats'
export { openAgentView, provideBranchView, useBranchView } from './state/branchView'
export type { BranchView } from './state/branchView'
export { trace } from './state/trace'
export type { TraceKind, TraceRow } from './state/trace'
export type {
  ActivityCatalog,
  ActivityRecall,
  ActivityRunning,
  ActivityStep,
  ActivityThought,
  ActivityTools,
  GroupActivityItem,
  GroupContextItem,
  GroupItem,
  GroupTextItem,
  TimelineCompactionEntry,
  TimelineContextEntry,
  TimelineEntry,
  TimelineGroup,
  TimelineUserEntry,
  ToolRowInfo,
} from './state/timeline'

export {
  beginDrawerClose,
  closeDrawer,
  dismissToast,
  openCardWindow,
  openDrawer,
  openLightbox,
  closeLightbox,
} from './state/ui'

export { findAgentChild, forkAt, selectBranch } from './state/tree'
export type { OpenHistory, PromptAttachment } from './state/sessions'

export {
  cardHistory,
  cardVersion,
  addFolder,
  createDeck,
  deleteCard,
  deleteDeck,
  deleteFolder,
  loadCard,
  moveEntry,
  moveOpenCard,
  newCard,
  openCard,
  openLibrary,
  openLibraryAtDeck,
  openedCard,
  refreshLibrary,
  renameDeck,
  saveCard,
  searchLibrary,
} from './state/library'
export type { OpenCard, SaveCardRequest } from './state/library'
export {
  answerDecision,
  closeSession,
  deleteSession,
  editPrompt,
  forkSession,
  forgetSession,
  goHome,
  showStats,
  showView,
  killTask,
  newSession,
  openSession,
  openTaskDetail,
  taskChild,
  compactBranch,
  defaultAssistant,
  regenerateFrom,
  retryBranch,
  revertFiles,
  revertPlan,
  retitleSession,
  refreshAssistants,
  refreshTasks,
  sendPrompt,
  startSessionIn,
  stopTurn,
  viewAssistant,
} from './state/sessions'

export { deleteWorkspace, refreshWorkspaces, saveWorkspace } from './state/workspaces'

export {
  chooseLandingAssistant,
  chooseLandingProject,
  createProject,
  deleteProject,
  folderProjectName,
  folderWorkspaceName,
  landingAssistant,
  landingProject,
  moveSession,
  refreshProjects,
  updateProject,
} from './state/projects'

export { startApp } from './state/pushes'
