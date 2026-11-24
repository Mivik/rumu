// Shared reactive state for the session UI. Pure data — no behavior here, so
// every mutation has an owner in one of the sibling modules.

import { reactive } from 'vue'
import type { PageRoute, SessionView, StatsRange } from '../router'
import type {
  Artifact,
  Branch,
  BranchId,
  Assistant,
  PendingDecision,
  Node,
  NodeId,
  PathState,
  SessionId,
  SessionInfo,
  SessionUsage,
  Tagged,
  TaskId,
  TaskInfo,
  ToastKind,
  LibraryDeck,
  LibraryCard,
  LlmDeltaPush,
  Project,
  Workspace,
} from '../types'

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
  /** Whether any reasoning / answer text has arrived. The timeline reads
   * these, not the text: reading the text would rebuild the whole timeline
   * — and with it every turn in the conversation — on every delta. */
  hasReasoning: boolean
  hasText: boolean
  /** Live thinking clock: when the first reasoning delta landed, and the
   * measured span once the first text delta arrives (null until then). */
  reasonStart?: number
  reasonMs?: number
  /** The number of the last delta folded in. */
  seq: number
  /** Full lengths of the two streams (the text above keeps only its tail),
   *  to drop the part of a merged push already held (its `at`). */
  textLen: number
  reasoningLen: number
  /** Rough tokens streamed so far (the text above keeps only its tail). */
  tokens: number
  /** Deltas of a stream joined midway, kept until the `session.tree`
   *  snapshot supplies its start (see `mergeLive`). */
  early?: LlmDeltaPush[]
}

/** An automatic retry of a model call in flight (`turn.retrying` push). */
export interface RetryState {
  attempt: number
  max: number
  /** Wall-clock ms the backoff ends at, for the live countdown. */
  until: number
  message: string
}

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
    /** Starts true: the tree is read once the page opens, so the first render
     *  must not claim the library is empty. */
    loading: true,
  },

  sessions: [] as SessionInfo[],
  /** The configured assistants (`assistant.list`); exactly one is the default. */
  assistants: [] as Assistant[],
  /** The assistant in view: whose sessions the sidebar lists, whose cards
   *  the shelf shows outside a session, and who new sessions run as. */
  assistant: '',
  /** Bumped whenever cards may have changed on disk: open card windows
   *  re-read theirs. */
  cardsRev: 0,
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
  /** The replies the stats view narrows to; null for the whole path. */
  statsRange: null as StatsRange | null,
  /** The sub-agent the stats view shows; null for the main line. */
  statsAgent: null as BranchId | null,
  /** The management page over the session, if any (mirrored in the
   *  address); the session stays loaded beneath it. */
  page: null as PageRoute | null,
  session: null as SessionInfo | null,
  nodes: {} as Record<NodeId, Node>,
  branches: {} as Record<BranchId, Branch>,
  /** Sub-agent branches whose nodes were fetched (`branch.nodes`):
   *  `session.tree` leaves them out until one is opened. */
  agentNodes: {} as Record<BranchId, 'loading' | 'loaded'>,
  /** A variant's newest end by its first node, for the variants the tree
   *  sent only that node of: where its switcher moves the head. */
  variantEnds: {} as Record<NodeId, NodeId>,
  /** Where the loaded stretch of the main line ends, when that is short of
   *  its head (a reload opened where it was read): the line is shown up to
   *  here, and what comes after loads as the view nears it. */
  lineEnd: null as NodeId | null,
  /** Bumped when the main line is shown at its end again (`goToEnd`). */
  endJumps: 0,
  /** Bumped when the main line is shown from somewhere else than its end
   *  (`goToStart`, `goToTurn`): the view opens at `node`, or at the top. */
  lineJump: { seq: 0, node: null as NodeId | null },
  /** How many user messages the main line has before a node, for the
   *  first node of each window loaded (they number the turns shown), and
   *  how many on all of it at the last load. */
  turnsBefore: {} as Record<NodeId, number>,
  turnsTotal: null as number | null,
  /** Windows that opened mid-turn (where the model woke on its own), by
   *  their first node, in the order they loaded: the turn's group keeps the
   *  oldest one's key as the rest of the turn loads above it. */
  seams: {} as Record<NodeId, number>,
  /** Token totals over every model call of the open session. */
  usage: { calls: 0, input: 0, cached: 0, output: 0 } as SessionUsage,
  /** Each sub-agent's totals over its own model calls. */
  agentUsage: {} as Record<BranchId, SessionUsage>,
  /** The branch the chat shows: the session's main line. */
  mainBranch: null as BranchId | null,
  pathState: null as PathState | null,

  /** Streaming buffers keyed by branch; cleared when the Assistant node is appended. */
  deltas: {} as Record<BranchId, DeltaBuffer>,
  /** Agent-loop-running flag keyed by branch (turn.started .. turn.done/error). */
  busy: {} as Record<BranchId, boolean>,
  /** Prompts accepted while a branch was busy, until their user node lands. */
  queuedPrompts: {} as Record<BranchId, { id: NodeId; text: string }[]>,
  /** Pending automatic retry of the model call, keyed by branch; cleared by
   * the next turn event or the branch's first delta. */
  retrying: {} as Record<BranchId, RetryState>,
  /** Branches whose context is being compacted (`compact.started` until
   * `compact.done`/`compact.failed` or the turn's end): since when (ms),
   * and the view's size before, tokens. */
  compacting: {} as Record<BranchId, { since: number; before: number }>,

  /** Live foreground tool output, keyed by `${branch}:${call_id}`; `at` is
   * when the last piece arrived (a call silent too long reads as stalled). */
  toolProgress: {} as Record<
    string,
    { branch: BranchId; call_id: string; text: string; data: Tagged | null; at: number }
  >,
  /** Kernel tasks of the open session (runtime-only; `task.list` + pushes). */
  tasks: {} as Record<TaskId, TaskInfo>,
  /** The task whose buffered output is open in the tasks panel. */
  taskDetail: null as TaskId | null,
  /** The sub-agent an open task output or artifact replaced beside the
   *  chat: their back button returns to it. */
  panelBack: null as BranchId | null,

  toasts: [] as Toast[],

  /** Open card windows, bottom first. Each is a native modal <dialog>, so
   *  the last one sits on top; closing it reveals the one below. */
  drawers: [] as CardWindow[],
  /** The image shown full-size (opened by any thumbnail). */
  lightbox: null as LightboxImage | null,
  /** The open artifact: the `show` result node it was recorded by, or a
   *  file a link opened that no `show` on the path recorded. */
  artifact: null as NodeId | Artifact | null,
  /** The sub-agent branch open beside the chat. */
  agentView: null as BranchId | null,
}

// Cast off Vue's `UnwrapNestedRefs`: the wire payloads embed a recursive JSON
// union, which exceeds TS's instantiation limit when deep-unwrapped here.
export const state = reactive(initial) as typeof initial
