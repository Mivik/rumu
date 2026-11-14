# Decks

A **deck** is a directory of cards that are mounted, shared and approved
together: the user, a project, a domain, an assistant. Decks are the only unit
of memory, permission and history. An **assistant** is a deck as well. Its pinned cards are its persona and
instructions, and its `deck.yaml` names the decks it mounts by default and
the [MCP servers](mcp.md) and [skills](skills.md) it uses. An assistant has no other configuration.

## Where a card belongs

A deck's charter, its pinned `about.md`, says what belongs in it. Who mounts a
deck is the user's decision, and an assistant that mounts a deck is meant to
know what is in it, so placement depends on the charter alone, never on who
else mounts the deck.

- Put a card in the mounted deck whose charter fits what the card is about,
  whichever assistant learned it: a fact about the user goes to the deck
  mounted at `/cards/user`, a fact about the session's project to
  `/cards/project` (see [Projects](projects.md)).
- Your own deck (`/cards/self`) is the assistant's own memory: its persona and
  instructions, what it learned from its own work (approaches that worked,
  mistakes to avoid), and work under way outside any project. A card no
  mounted deck fits also stays there until one does; then move it with
  `write` and `from`.
- Isolation is a mount decision, not a placement one. An assistant that must
  not know the user (a roleplay persona, say) mounts a different deck at
  `/cards/user`; the rule above still holds there.
- Process stays in sessions (`@rumu:/sessions` is memory too). Only
  conclusions become cards.
- Link related cards instead of copying them.
- A deck's structure lives in its charter: adding a directory or changing what
  one holds updates the charter (what each directory holds, what to read
  first). There is no separate index card; the catalog lists the cards.

The first-call note states these rules.

## Decks

- A deck is a bare git repository at `RUMU_HOME/decks/<id>/`. Its `name` is a
  display label — trimmed, 1–80 characters, no control characters, unique
  among decks (case-insensitively) — and never a path or a lookup key.
- **rumu is the only writer.** A deck has no worktree: its content is the
  `HEAD` tree, and every change (a card write, a UI edit) is a
  commit made by rumu. Hand edits are not supported, so there is nothing to
  scan or reconcile.
- `deck.yaml` holds the deck's identity and, for assistants, their defaults:

  ```yaml
  id: 3f6c2a91-5d4e-4b7a-9c01-8e2f6a4b1d30   # UUIDv4, assigned by rumu
  name: ledger
  assistant: true            # listed in the roster; sessions can run as it
  private: true              # optional: hidden from agents unless mounted
  mounts:                    # mounted at the start of each of its sessions
    - { deck: <deck-id>, at: /cards/user }              # writes ask the user
    - { deck: <deck-id>, at: /cards/notes, write: true } # writes need no approval
  allow_ask: [<deck-id>]     # these assistants may ask it without a prompt
  mcp: { github: ["*"] }     # MCP servers in settings.yaml, with the tools enabled
  skills: ["*"]              # the skills enabled (docs/skills.md)
  cat: { items: [bow, blush], tone: pine, wide: 1, ears: -2 }  # its mascot; left out, drawn from the id
  ```

- **Private decks.** A deck with `private: true` is visible to an agent only
  through the session's mount table: mounted on the path, the session's own
  deck, or listed in its assistant's `deck.yaml` mounts. Only the user puts it
  there (creating a session in its project, dragging a session into it, or
  editing the assistant's mounts). A private deck is skipped in `card_mount`'s
  listing, and mounting it by id is refused exactly like an unknown id, before
  any approval (an "always allow" grant does not help). `ask` to a private
  assistant likewise fails as unknown, and the first-call note does not name
  it.
- **Private links.** A link into a private deck from a different deck neither
  resolves nor can be created: it shows as dangling and writing it is refused.
  Links within the private deck itself are ordinary links.
- Everything refers to a deck by id: grants, session roots, mounts, links, and
  the model's `card_mount` and `ask` arguments (`card_mount` with no arguments
  lists the ids). Names are for display only and are never looked up.
  Renaming a deck changes `name` and nothing else.
- Decks are created by the user in the web UI. An assistant creates only
  project decks, with the user's approval (see [Projects](projects.md)). A new
  deck starts with a blank charter (see [Cards](#cards)).
- A home with no decks at all starts with three: `user` (the user), `world`
  (facts and methods that are neither about the user nor tied to one
  project), and the default assistant `default`, which mounts the other two
  at `/cards/user` and `/cards/world` with writes allowed. Each has its
  charter filled in.
- `deck.yaml` and the cards are in the tree; grants are per user and stay in
  `RUMU_HOME/grants.json` (see [Approvals](#approvals)).
- The default assistant is a per-user pointer: `RUMU_HOME/default-assistant`
  holds its deck id, an ordinary UUIDv4. A session runs as it when no
  assistant is named. Any assistant can be made the default, and any can be
  removed from the roster. When the pointer does not name an assistant deck,
  rumu points it at the first assistant deck by name on load; when there
  is none, it creates one (named `default`, stored under its id).

## Cards

- A card is a `.md` file in a deck. Its path within the deck is its location,
  not its identity.
- **Identity.** The frontmatter `id:` is a UUIDv4 and is unique across all decks.
  It lives in the file so that the identity survives moves and clones by
  collaborators.
  - rumu assigns it when it creates a card.
    Every card in a deck has a valid id and frontmatter, because only rumu
    writes the tree.
  - **The model never sees ids.** It addresses cards by path only. The id is
    hidden from the editable view and from `view_rev`. An `id:` in written content
    is ignored, and an overwrite keeps the card's id. `read` attaches the path and
    id as structured data (`CardReadHeader`) for the UI, outside the text.
- **Pinned.** A card with `pinned: true` is loaded into context when its deck is
  mounted; a blank one is skipped. Like any card, it can be written by whoever
  may write its deck, so an assistant can maintain its own persona.
- **Charter.** The pinned `about.md` says what belongs in the deck and what
  does not. Its `description` (or, without one, its first body line) is the
  deck's one-line summary, shown wherever decks are listed: `card_mount` with
  no arguments, the other assistants in the first-call note, and the library.
  `deck.yaml` has no description of its own, so the model can keep the
  charter current like any other card.
- **Links.** A link card has `link: @rumu:/cards/<path>` in its frontmatter. The
  link file holds only `id` and `link`: other fields and the body are dropped on
  write, and every view (catalog, `read`, recall, library) shows the target's.
  A link is never pinned; its target's deck pins it.
  - On write, rumu resolves the path to the target's id and stores the id. The
    model's view shows the target's current mount path, or `<deck>:<path>` when the
    target's deck is not mounted. Moving the target does not break the link.
  - Reading a link card shows the target's content, under a header that names the
    target's location.
  - Writes through a link are refused. The target's deck decides who may write
    it.
  - Links are one level deep: linking to a link card is refused, and so is
    turning a card that others link to into a link.
  - A dangling target is marked in the catalog and in `read`.
  - Recall counts a card once, even when it is reachable both directly and
    through a link.
  - Creating a link to a card in another deck needs the user's approval
    (`link`, see [Approvals](#approvals)). The exception is a target in the
    link's own deck. A link shares its target with every session that mounts
    the link's deck.
- **Body links.** A card body links other cards by Markdown links relative
  to the card (`[deploy](../ops/deploy)`, `.md` optional), resolved within the
  card's own deck. Mount paths (`@rumu:/cards/…`) are not stable in a body: the
  same deck is mounted at different points by different sessions. A card in
  another deck is reached through a link card in this deck.
  - A `write` lists the body's relative links that name no card in the deck
    and its mount-path links. The write itself is not refused.
  - The UI opens relative links within the card's deck in a new card window
    over the one showing it, so the card they were reading stays open below.
    With nothing showing, a mounted card opens in the drawer and an unmounted
    one in the library.

## Mounts

- `/cards` is empty until decks are mounted. A session starts with its own deck
  at `/cards/self` and the mounts listed in its `deck.yaml`.
- Mounting makes a deck readable. It has no mode: writing is approved
  separately, per deck (see [Approvals](#approvals)).
- `card_mount` manages mounts:
  - With no arguments, it lists decks: name, id, mount point if mounted,
    whether mounting or writing it would ask the user, and its summary.
  - With `deck` and `at`, it mounts a deck (`mount` approval).
  - With `at` alone, it unmounts.
- The mount table is branch state on the node path, so forks and sub-agents
  inherit it, the session's project included. Mount points must not overlap:
  neither may be a prefix of the other. `/cards/self` can be neither mounted
  nor unmounted, and `/cards/project` holds only project decks.
  `deck.yaml` mounts follow the same rules; invalid entries are refused when
  saved.
- Mounting returns, at that point in the conversation, the deck, its pinned
  cards and its [catalog](#catalog). The prompt cache before that point is
  unaffected.
- Unmounting inserts a note saying that `/cards/<at>` is unmounted and that
  content loaded from it earlier is no longer readable, writable or updated.
  Unmounting only affects what happens after it.
- Only mounted decks take part in the catalog, recall, `list`, `search` and
  update notices.

## Catalog

- Each mounted deck has its own catalog. The first model call of a path that
  has no catalog yet gets one note per mounted deck (the session's own deck and
  its `deck.yaml` mounts). `card_mount` returns the mounted deck's catalog in
  its result. A catalog is frozen: later changes to the deck do not rewrite it
  (update notices cover the cards in use).
- A catalog lists the deck's 30 cards with the highest use score, in path
  order, as links with their summary. Pinned cards are left out: they are
  loaded in full beside it. The rest fold into one line per
  directory, directories ordered by total score: the directory, its card count
  and its first names by score. `list` and `search` reach folded cards, and
  recall brings relevant ones in.
- **Usage index.** Each deck keeps `usage.json` in its repository directory,
  outside the tree, so it is never committed or synced. Per card id it holds a
  score and the time of its last update.
  - A successful `read` of the card by any assistant adds 1. A commit touching
    the card adds 3, whoever the author (UI edits included). Recall, search
    hits and pinned loading do not count. A read through a link counts for the
    target. Deleting a card or moving it to another deck drops its entry.
  - Scores decay with a 30-day half-life, updated in place:
    `score · 2^(−Δt / 30 days) + w`.
  - Keyed by id, a score follows moves within the deck. A missing or
    unreadable file starts every card at 0; ties go by path.

## Approvals

An assistant acting on a deck other than its own asks the user first
(`decision.requested`), unless a grant already covers it. There are five
actions:

| action   | what                                    | granted up front by                    |
|----------|-----------------------------------------|----------------------------------------|
| `mount`  | mount the deck (makes it readable)      | an entry in its `deck.yaml` mounts     |
| `write`  | write, move into or out of its cards    | a `deck.yaml` mount with `write: true` |
| `ask`    | ask the assistant deck a question       | `allow_ask` in the asked deck          |
| `link`   | link one of its cards from another deck | nothing                                |
| `create` | create a [project](projects.md) deck    | nothing                                |

- An assistant may do anything with its own deck and with the session's
  project deck.
- The user answers with one of: don't allow, allow once, allow for this
  session, always allow.
  - "Allow for this session" covers the rest of the session, its branches
    and sub-agents included; a session forked from it asks again. It is kept in
    memory, so after a restart rumu asks again.
  - "Always allow" is stored in `RUMU_HOME/grants.json` as
    `{ <action>: { <assistant deck id>: [<deck id>, …] } }`. Revoke it by
    editing the file, which is read on every check.
- A write request shows the deck, the path, the source of a move and the
  content. A move across decks is approved for both decks.
- A session answering an `ask` cannot write cards at all.

## Writing

- **Read before write (CAS).** As today, a write must be based on the card's
  current `view_rev`, which the branch path records from the last read. The
  check is repeated under the store's write lock, so of two writes based on the
  same read only one lands. When the
  write is refused, the refusal lists each change since the seen `view_rev`
  (author, session and time) and the diff from the seen version to the current
  one. The model re-reads and rewrites. There is no automatic merge, because
  merged text can keep contradictory facts side by side without anyone noticing.
- **Move.** `write` with `from: <path>` writes the new path and removes `from`,
  keeping the card's id. Card paths, `from` included, are validated: no `..`,
  no hidden segments, no bare mount point.
  - The destination must not exist.
  - `from` must pass the read check.
  - Moving across decks needs the `write` approval of both decks.
- `deck.yaml` is never written by the model.

## History

History is git. There is no other log.

- **One commit per write.** rumu commits each card write, move or link write on
  its own.
  - The message is generated, for example `update notes/storage`.
  - The author is the writing assistant (`<deck name> <<deck-id>@rumu>`), or
    `user` (UI edits).
  - Trailers record `Card: <id>` (one per card touched), `Path: <id> <path>`
    (that card's file at this commit), `Session:` and `Node:`.
  - History by identity is `git log --grep "Card: <id>"`, so it follows moves
    without relying on git's similarity-based rename detection.
- **Moves across decks** touch two repositories. rumu commits the destination
  first and then the deletion in the source. A crash in between leaves the
  card in both decks. On load, rumu finishes such a move: of two copies of an
  id, the one whose latest commit is newer (the destination) stays, and the
  other is deleted in a commit by the same author. Nothing is lost.
- **CAS refusal.** A card use records the `HEAD` commit it was read at, next to
  `view_rev`. A refused write lists the commits since then that touch the card
  (author, session, time) and the diff from the version that was read to the
  current one.
- **Reading history.** The model reads history through the `@rumu` workspace,
  by path:
  - `list @rumu:/history` is the timeline of the mounted decks, newest first,
    one line per change: short commit, path, op, author and time.
  - `list @rumu:/history/cards/<path>` lists a card's versions, following its id
    across moves.
  - `read @rumu:/history/cards/<path>@<commit>` shows that version's header and
    its diff from the previous version.
  - A deleted or moved-away card keeps its old path here: `list` and `read`
    under `@rumu:/history/cards/<old path>` find it in the history of the deck
    mounted there. A `read` of a commit that does not touch the card is refused.
- Catalog lines name the last author when it is not the current assistant.
- No git binary is required: rumu reads and writes the repositories with
  gitoxide (`gix`). Push is not covered by `gix`.

## Sessions

- Every session runs as one assistant deck, whose id is recorded on its root node.
  Forks and sub-agents inherit it. Sessions stay disposable.
- The sidebar groups sessions by project (see [Projects](projects.md)). A new
  session picks the assistant it runs as.
- `@rumu:/sessions` shows the sessions of every mounted deck: a conversation
  is part of its project deck's memory, or of its assistant deck's when it has
  no project, so mounting the deck opens it, read-only like all sessions.
  Lines for another assistant's sessions name it, and the line of the session
  the call runs in is marked `(current)`. `@rumu:/sessions/by-assistant` lists
  the assistants whose sessions are visible, and
  `@rumu:/sessions/by-assistant/<deck id>` lists one assistant's visible
  sessions; it is also a `search` scope. An assistant with no visible
  sessions is refused.
- The first model call of a path gets a one-time note: which assistant this is,
  the project and its workspaces, the mount table, the placement rules above,
  and the pinned cards of the mounted decks. It is frozen as a note, but
  pinned cards stay current: when one changes (an assistant's instructions
  edited mid-session, say), or a mounted deck has one the branch has not
  seen, the next model call gets its text in full.

## Asking another assistant

`ask(to, questions)` asks the user (`to: "user"`) or another assistant deck.
The user gets up to four questions at once, each with optional options (a
label and a description) and always room for their own words, through a
`questions` decision. Another assistant gets exactly one question, without
options:

1. Assistant A calls `ask(to: "<ledger's id>", questions: [{ question: "..." }])`.
2. Unless `allow_ask` in B's `deck.yaml` or an "always allow" in `grants.json`
   permits it, the user is asked (`decision.requested`).
3. B answers in a new session of its own, with its default mounts and its own
   deck, and cannot write cards there. The root records `asked_by: <A's id>`; `ask`
   and `card_mount` are refused on that session. `to` must be an assistant
   deck's id; the first-call note lists the other assistants with their ids.
4. The answer returns to A as the tool result. Nothing is saved automatically.

## Not in scope

- Automatic routing of a message to an assistant.
- Several assistants in one conversation.
- Automatic merging of concurrent edits.
- Permissions inside a deck: split it into two decks instead.
