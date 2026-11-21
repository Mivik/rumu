# Projects

A **project** is what a session works on: its workspaces (directories) and its
memory. A session runs as one assistant (who) in at most one project (what).
An assistant works in any project.

## Project decks

- A project is a deck with `project: true` in its `deck.yaml`. Its charter
  says what the project is, and its cards are the project's memory. A deck is
  not both an assistant and a project.

  ```yaml
  id: 7b1e4c20-9a3f-4d8e-b6c5-2f0a9d1e3c47
  name: rumu
  project: true              # listed in the sidebar; sessions can work in it
  icon: { glyph: 论, tone: memory }  # its seal; optional, as is each part
  ```

- A project's seal is its mark wherever it shows (its card, its sidebar
  group, the pickers, the chat header): a glyph (a letter, a CJK character or
  an emoji) on one of six tones (`brand`, `tool`, `pine`, `memory`, `agent`,
  `ink`). An unset glyph is the name's first letter; an unset tone is picked
  from the id among the first five. It lives in `deck.yaml` because it travels
  with the deck.

- The user creates a project in the web UI, either from a folder (the project
  is named after it and the folder is mounted) or blank. An assistant creates
  one with the user's approval (see [Agent control](#agent-control)).

## Workspaces

- A **workspace** is a named directory, local or on an ssh host, in the
  registry (`RUMU_HOME/workspaces.yaml`, written only by rumu). Files are
  addressed as `<name>:<path>`, and `@rumu` holds rumu's own data.
- `@scratch` is the session's own directory at
  `RUMU_HOME/scratch/<session id>`, created on first use and removed with the
  session. Its branches and sub-agents share it. It is local only, never
  synced or versioned: use it for throwaway files. Like `@rumu`, it is always
  addressable, even when an `agent`'s `workspaces` narrows a sub-agent.
- Every registered workspace is addressable from every session. Attaching
  was never a sandbox (`bash` in a local workspace reaches the whole
  machine), so sessions keep no list of attached workspaces. An `agent`'s
  `workspaces` still narrows what a sub-agent may address.
- A project **mounts** workspaces: an ordered list of names, the first being
  the project's default. A workspace may be mounted in several projects (a
  read-only upstream checkout, say).
- The lists are local to this machine, in `RUMU_HOME/projects.yaml`
  (`<deck id>: [<name>, …]`, written only by rumu) beside the registry, so
  they are never committed. A collaborator who clones the deck mounts their
  own directories. Names that leave the registry drop out, and deleting the
  deck drops its list.
- The default workspace of a path is the last `workspace.use` marker, else
  the first of its project's workspaces it may address, else `@scratch`.
  Entering, switching or leaving the project drops the `workspace.use` before
  it, so the new project's first workspace (or `@scratch`) is the default;
  the result or note of the move says so when the default changes.

## Sessions in a project

- A session is in **at most one** project, its own. Its project deck is
  mounted at `/cards/<name>` (the name made path-safe, `-2`, `-3`, … when
  taken), and the mount marker says it is the project (`project: true`). A
  path has at most one such mount.
- Any other deck mounts beside it, another project's deck too: a plain mount
  (`project: false`) that leaves the session's project, its sidebar group and
  its default workspace as they are. Its cards are writable as any mounted
  deck's, with the `write` approval, and the mount lists its workspaces
  (every registered workspace is addressable anyway). This is how a session
  works across two projects: it belongs to one and borrows the other.
- A session starts in the project it was created in. Created without one, a
  session of an assistant whose `deck.yaml` mounts a project deck is in that
  project (the first such mount); created in another project, that default
  mount stays a plain one.
- `deck.mount` with `project: true` moves the session into a project deck
  from that point on: the old project is unmounted first, and a deck mounted
  already becomes the project where it is. Unmounting the project leaves it.
  Like every mount, it is a marker on the branch, so forks inherit it.
  Besides the deck, its pinned cards and its catalog, the mount returns the
  project's workspaces.
- A fresh `agent` and `session.create` start in the caller's project, at the
  point the caller has it; the decks the caller only borrows stay its own.
- The first-call note names the project, where it is mounted, and lists its
  workspaces (name, location, read-only), the default first.
- A session is listed under the project of its main line.
- Journals from before this mount their project at `/cards/project` with
  markers that do not say `project`; a mount there reads as the project and
  stays there. A `project: true` mount while a path already has a project
  reads as a plain mount.

## Memory boundary

- Facts about a project and work under way on it go to its deck. What spans
  projects stays with the assistant's mounts: the user (`/cards/user`),
  general knowledge (`/cards/world`) and the assistant's own lessons
  (`/cards/self`).
- A session's conversation belongs to one deck: its project's, or its
  assistant's when it has no project. Mounting a deck opens its sessions,
  as the project or borrowed, so `@rumu:/sessions` in a project session lists
  that project's sessions, whichever assistant ran them, and the assistant's
  sessions outside any project. Another project's sessions are listed only
  while its deck is mounted (a private one's too).
- The sidebar groups sessions by the same rule: a project's group is exactly
  the sessions it is the project of.

## Agent control

The session's project is the assistant's to manage, like its own deck: its
cards and its workspaces.

Configuration is in the `@rumu` workspace and is changed with the file
tools. What can be done to one of its objects beyond reading and writing it
is one of the object's actions, run with `invoke { action, path, args? }`:
`action` is the full name (`<type>.<action>`, unique), `path` the object, and
`list` on an object shows its actions with their arguments.

`@rumu` documents itself, so the system prompt only points at `list
@rumu:/`. `list` on any of its paths shows, a blank line apart: what the
path holds, what may be written there and what asks the user (its node's
doc); its entries; `files:`, the files rumu defines under it, each with a
line (a task's `output`); and `actions:`. `list @rumu:/` names the roots, a
line each. A path gets a file of its own only for a part with its own size
or use: `@rumu:/tasks/<id>` is a task's summary (its state, the last lines
of its output, the start of its result) and `@rumu:/tasks/<id>/output` its
whole buffered output, which `bash` watch events number their lines in. A
task's record lives in its session's journal, so a restart of rumu keeps the
table: a task that was still running comes back as `lost`, with a note
saying so, rather than quietly vanishing.
`search` marks each hit `card`, `session` or `action`.

Which set a workspace is in shows in its path, not in a field:

```
@rumu:/workspaces/<name>      every visible workspace (the registry)
@rumu:/decks/<name>           a deck's configuration; a project's has its
                              mounted workspaces (`workspaces:`)
```

- `list @rumu:/workspaces` lists the workspaces: the project's first, then
  the rest of the registry, then `@scratch` and `@rumu`; each with its
  location, marked when it is mounted in the session's project or is the
  default. `read @rumu:/workspaces/<name>` returns its entry, `write` creates
  one:

  ```yaml
  root: ~/src/tokio
  ssh: build@ci.example   # optional: a remote workspace
  read_only: true         # optional
  ```

  A local `root` must be an existing directory. Changing an existing entry
  asks the user (`workspace`), since other projects may mount it, and so does
  `delete`, which also unmounts it from every project.
- A project's `workspaces:` in `@rumu:/decks/<name>` is an ordered list of
  registered names, the first being the project's default. Writing it mounts,
  unmounts and reorders. Like a card, the deck must be read before it is
  written, so two sessions of the project cannot overwrite each other's
  change. (Its other fields and the other kinds of deck: see
  [Decks](decks.md#agent-configuration).)
- Mounting is a property of the project, not of the workspace, so it is not
  kept under `@rumu:/workspaces`. The registry listing only shows it.
- `invoke { action: "workspace.use", path: "@rumu:/workspaces/<name>" }` sets
  the branch's default and lists the workspaces as they then are. `@scratch`
  and `@rumu` have entries there for this alone (they cannot be written or
  deleted). It is the web's `branch.use_workspace`: the same `workspace.use`
  marker.
- `invoke { action: "deck.mount", path: "@rumu:/decks/<name>" }` mounts a
  deck (`args: { at }` picks the point); a project deck stays borrowed.
  `args: { project: true }` moves the session into it instead, and
  `deck.unmount` on the project's deck leaves it. `list @rumu:/decks` names
  each deck's kind. No web method is called `deck.mount` or `deck.unmount`:
  the web moves a session with `session.move { session, project }` (`null`:
  out of its project), which writes the same markers with a note each.
- Writing `kind: project` (and, optionally, `workspaces:`) to a new
  `@rumu:/decks/<name>` creates a project deck with a blank charter; the
  assistant then writes the charter and mounts it with `deck.mount`, which
  does not ask again in that session.
- An action's full name that is also the name of a web (WebSocket) method
  names the same operation, as `task.kill` (cancel a background task) does;
  the model's may only reach its own branch's objects, and asks where the
  web does not. An operation that differs gets another name.

| what                                          | approval            |
|-----------------------------------------------|---------------------|
| register a workspace                          | none                |
| change a registered workspace                 | `workspace`         |
| delete a registered workspace                 | `workspace`         |
| mount, unmount or reorder the project's ones  | none                |
| the same in another project                   | `configure`         |
| write the session's project deck              | none                |
| leave the project                             | none                |
| move into an existing project, or borrow it   | `mount` of its deck |
| write a borrowed project deck                 | `write` of its deck |
| create a project                              | `create`            |

- `create`, `configure` and `workspace` are approval actions like those in
  [Approvals](decks.md#approvals). "Always allow" is stored by target:
  `{ "workspace": { <assistant deck id>: [<workspace name>, …] } }`, and
  `create`, which has none, with `*`.

## Web UI

- The sidebar is a two-level tree: a group for each project, and one for
  each assistant holding its sessions without a project (headed by its
  avatar), each with its sessions, newest first. Groups with sessions are
  ordered by latest activity, projects and assistants alike; the empty
  projects follow by name. A session row in a project group names its
  assistant unless it is the default one.
- Each group row has an action to start a session in it (a project group's
  also opens its settings). Dragging a session to another project's group
  moves it there, and to its own assistant's group out of its project: the
  same markers as `deck.mount`, with no approval.
- The chat header shows the session's project, if any, beside the view tabs,
  with a menu to move it to another project or out of its own.
- The projects page is a grid of project cards: a contour band drawn from
  the id under the seal, the name, the workspaces (left out when the only one
  shares the project's name), the latest activity and a new-session action.
  The workspace registry is a compact list below it.
- One dialog creates a project and edits one: the seal (beside the name: a
  tone and a glyph), the name, a Private toggle (hidden from agents unless
  the session mounts it; see [Decks](decks.md)), the charter (an existing
  project's), and the workspaces: add a folder (registered, named
  after it, when the dialog is submitted) or, secondarily, a registered
  workspace; a new project takes the first one's name; reorder (the first is
  the default), remove. A new project is where the next new session starts.
- A new session picks one assistant and one project, by default the project
  in view and the assistant last used in it. Assistants are listed in that
  picker and managed in the library.
- The composer has no workspace picker.

## Fresh home

- With no `workspaces.yaml`, rumu registers the current directory and
  creates a project named after it, so it runs out of the box. Started from
  the user's home directory, it registers the workspace but creates no
  project.

## Replaces

- `workspaces` in `deck.yaml`, and the default assistant attaching every
  workspace: assistants carry no workspaces.
- The attached list the user sets per branch: the composer's workspace
  panel, the `workspaces` of a prompt's choice and `prefs.workspaces`. The
  `workspace.set` marker stays only for an `agent`'s `workspaces`.
- The assistant roster as the sidebar's top level: the roster lives in the
  new-session picker.

## Not in scope

- A project's own default mounts (`mounts` in a project deck's `deck.yaml`).
- Several projects of one session: a session borrows other project decks
  instead (see [Sessions in a project](#sessions-in-a-project)).
- An `ask` session in the asking session's project.
