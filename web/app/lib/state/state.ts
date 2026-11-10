// Shared reactive state for the session UI. Pure data — no behavior here, so
// every mutation has an owner in one of the sibling modules.

import { reactive } from 'vue'
import type { PageRoute, SessionView } from '../router'
import type {
  Decision,
  Effect,
  Branch,
  BranchId,
  Card,
  Assistant,
  PendingDecision,
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
  LlmDeltaPush,
  Project,
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

/** One open card window. Windows stack: a body link opens a new one over the
 *  card it was clicked in, instead of replacing it. */
export interface CardWindow {
  /** Stable identity of the window in `state.drawers`. */
  key: number
  /** The mounted card id when the window was opened from a chip (shown as
   *  the address); null for a card reached by a deck-local link. */
  cardId: string | null
  /** The card's deck and deck-local path; null only when a chip's id could
   *  not be resolved (an unmounted or deleted card). */
  deck: string | null
  path: string | null
  /** The use the window was opened from (a note node id or a tool call id):
   *  the reader defaults to the "as seen" version when it is stale. */
  useRef?: string
  /** Animating closed: the entry leaves the stack a moment later, but the
   *  window below already takes over the scrim. */
  closing?: boolean
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
  /** The number of the last delta folded in. */
  seq: number
  /** Deltas of a stream joined midway, kept until the `session.tree`
   *  snapshot supplies its start (see `mergeLive`). */
  early?: LlmDeltaPush[]
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
  /** The card library page (the open card is the page's item). */
  library: {
    decks: [] as LibraryDeck[],
    cards: [] as LibraryCard[],
    query: '',
    /** Server search hits for `query` (null while the query is empty). */
    hits: null as LibraryCard[] | null,
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
  /** The project decks (`project.list`, kept live by `workspaces.updated`). */
  projects: [] as Project[],
  /** The workspace registry (`workspace.list`, kept live by `workspaces.updated`). */
  workspaces: [] as Workspace[],
  /** Questions tool calls are waiting on, oldest first. */
  decisions: [] as PendingDecision[],

  // Open session tree (from session.tree, kept live via pushes).
  sessionId: null as SessionId | null,
  /** The boot request is still opening the session the address names. The
   *  landing pane holds back rather than flashing before it arrives. */
  booting: false,
  /** How the open session is shown (mirrored in the address). */
  view: 'chat' as SessionView,
  /** The management page over the session, if any (mirrored in the
   *  address); the session stays loaded beneath it. */
  page: null as PageRoute | null,
  session: null as SessionInfo | null,
  nodes: {} as Record<NodeId, Node>,
  branches: {} as Record<BranchId, Branch>,
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

  /** Open card windows, bottom first. Each is a native modal <dialog>, so
   *  the last one sits on top; closing it reveals the one below. */
  drawers: [] as CardWindow[],
  /** The image shown full-size (opened by any thumbnail). */
  lightbox: null as LightboxImage | null,
  /** The open artifact: the `show` result node it was recorded by. */
  artifact: null as NodeId | null,
  /** The sub-agent branch open beside the chat. */
  agentView: null as BranchId | null,
}

// Cast off Vue's `UnwrapNestedRefs`: the wire payloads embed a recursive JSON
// union, which exceeds TS's instantiation limit when deep-unwrapped here.
export const state = reactive(initial) as typeof initial
