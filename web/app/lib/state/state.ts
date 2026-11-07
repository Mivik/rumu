// Shared reactive state for the session UI. Pure data — no behavior here, so
// every mutation has an owner in one of the sibling modules.

import { reactive } from 'vue'
import type {
  Decision,
  Effect,
  Branch,
  BranchId,
  Card,
  Assistant,
  PendingDecision,
  CodeCall,
  Node,
  NodeId,
  PathState,
  SessionId,
  SessionInfo,
  Tagged,
  TaskId,
  TaskInfo,
  Timestamp,
  ToastKind,
  LibraryDeck,
  LibraryCard,
  Workspace,
} from '../types'

export interface HookTraceEntry {
  seq: number
  at: Timestamp
  /** Session this entry was attributed to (the open session, or null if unknown). */
  session: SessionId | null
  branch: BranchId
  event: string
  plugin: string
  hook_id: string
  effects: Effect[]
  decision: Decision
  /** Enclosing `code` call id, for internal calls. */
  parent_call?: string
}

export interface Toast {
  id: number
  kind: ToastKind
  message: string
  /** Raw error text, shown under the message. */
  detail?: string
}

/** The image the lightbox shows: where its bytes are, and what to caption
 *  it with (dimensions only once the server has stored it). */
export interface LightboxImage {
  src: string
  name?: string | null
  width?: number
  height?: number
}

export interface DeltaBuffer {
  text: string
  reasoning: string
  /** Live thinking clock: when the first reasoning delta landed, and the
   * measured span once the first text delta arrives (null until then). */
  reasonStart?: number
  reasonMs?: number
}

export interface TurnCache {
  nodeId: NodeId
  index: number
  prompt: number
  cached: number
  creation: number
  completion: number
}

/** An automatic retry of a model call in flight (`turn.retrying` push). */
export interface RetryState {
  attempt: number
  max: number
  /** Wall-clock ms the backoff ends at, for the live countdown. */
  until: number
  message: string
}

export const MAX_TRACE = 500
export const MAX_DELTA_CHARS = 400_000

const initial = {
  /** The full-page card library dialog. */
  library: {
    /** Whether the library dialog is open. */
    isOpen: false,
    decks: [] as LibraryDeck[],
    cards: [] as LibraryCard[],
    query: '',
    /** Server search hits for `query` (null while the query is empty). */
    hits: null as LibraryCard[] | null,
    /** The open card: {deck, path}; `isNew` starts a blank form at `path`
     *  (a folder prefix, or `""`). */
    open: null as { deck: string; path: string; isNew?: boolean } | null,
    /** A deck to scroll into view the next time the tree renders (set by
     *  "open in library" jumps from elsewhere). */
    focusDeck: null as string | null,
    /** Expanded tree nodes (`deck:<id>`, `dir:<id>:<path>`). */
    expanded: new Set<string>(),
    /** Folders made in the tree that hold no card yet (`<deck id>:<path>`);
     *  a folder only exists on disk through its cards. */
    folders: new Set<string>(),
    loading: false,
  },

  sessions: [] as SessionInfo[],
  /** The configured assistants (`assistant.list`); exactly one is the default. */
  assistants: [] as Assistant[],
  /** The assistant in view: whose sessions the sidebar lists, whose library `cards`
   *  holds, and who new sessions run as. */
  assistant: '',
  cards: [] as Card[],
  /** The workspace registry (`workspace.list`, kept live by `workspaces.updated`). */
  workspaces: [] as Workspace[],
  /** Questions tool calls are waiting on, oldest first. */
  decisions: [] as PendingDecision[],

  // Open session tree (from session.tree, kept live via pushes).
  sessionId: null as SessionId | null,
  /** The boot request is still opening the session the address names. The
   *  landing pane holds back rather than flashing before it arrives. */
  booting: false,
  session: null as SessionInfo | null,
  nodes: {} as Record<NodeId, Node>,
  branches: {} as Record<BranchId, Branch>,
  /** Internal `code` calls of the open session, grouped by `code` call id. */
  codeCalls: {} as Record<string, CodeCall[]>,
  activeBranch: null as BranchId | null,
  pathState: null as PathState | null,

  /** Streaming buffers keyed by branch; cleared when the Assistant node is appended. */
  deltas: {} as Record<BranchId, DeltaBuffer>,
  /** Agent-loop-running flag keyed by branch (turn.started .. turn.done/error). */
  busy: {} as Record<BranchId, boolean>,
  /** Prompts accepted while a branch was busy, until their user node lands. */
  queuedPrompts: {} as Record<BranchId, { id: number; text: string }[]>,
  /** Pending automatic retry of the model call, keyed by branch; cleared by
   * the next turn event or the branch's first delta. */
  retrying: {} as Record<BranchId, RetryState>,
  /** A branch's turn failed terminally (a recorded `turn.error`, never a
   * hook-refused prompt), keyed by branch. Live only: never rebuilt from
   * stored markers, so a reloaded history shows no failure row; cleared when
   * the branch's next turn starts or ends, and when a session is opened. */
  failedTurns: {} as Record<BranchId, string>,

  /** Live foreground tool output, keyed by `${branch}:${call_id}`. */
  toolProgress: {} as Record<
    string,
    { branch: BranchId; call_id: string; text: string; data: Tagged | null }
  >,
  /** Kernel tasks of the open session (runtime-only; `task.list` + pushes). */
  tasks: {} as Record<TaskId, TaskInfo>,
  /** Live task output buffer (`Progress::Append` text), keyed by task id. */
  taskOutput: {} as Record<TaskId, string>,
  /** The task whose buffered output is open in the tasks panel. */
  taskDetail: null as TaskId | null,

  hookTrace: [] as HookTraceEntry[],
  toasts: [] as Toast[],

  /** Card drawer (opened by any card chip): card id + the use it was opened
   * from (a note node id or a tool call id; absent for the library). */
  drawer: null as { cardId: string; useRef?: string } | null,
  /** The image shown full-size (opened by any thumbnail). */
  lightbox: null as LightboxImage | null,
}

// Cast off Vue's `UnwrapNestedRefs`: the wire payloads embed a recursive JSON
// union, which exceeds TS's instantiation limit when deep-unwrapped here.
export const state = reactive(initial) as typeof initial
