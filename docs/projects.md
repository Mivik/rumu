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
  the project's first workspace, else `@scratch`. Entering a project makes its
  first workspace the default; leaving it returns to `@scratch`.

## Sessions in a project

- A session's project is the project deck mounted at `/cards/project`. Only
  project decks mount there, and a project deck mounted elsewhere is only
  read. `deck.yaml` mounts cannot use `/cards/project`.
- A session starts in the project it was created in, or in none.
- Mounting a project deck at `/cards/project` moves the session into that
  project from that point on; unmounting it leaves the project. Like every
  mount, it is a marker on the branch, so forks inherit it. Besides the deck,
  its pinned cards and its catalog, the mount returns the project's
  workspaces.
- An `ask` session has no project and keeps its mounts, so it never gets
  one.
- The first-call note names the project and lists its workspaces (name,
  location, read-only), the default first.
- A session is listed under the project of its active branch.

## Memory boundary

- Facts about the project and work under way on it go to `/cards/project`.
  What spans projects stays with the assistant's mounts: the user
  (`/cards/user`), general knowledge (`/cards/world`) and the assistant's own
  lessons (`/cards/self`).
- A session's conversation belongs to one deck: its project's, or its
  assistant's when it has no project. Mounting a deck opens its sessions, so
  `@rumu:/sessions` in a project session lists that project's sessions,
  whichever assistant ran them, and the assistant's sessions outside any
  project. Another project's sessions are listed only while its deck is
  mounted.
- The sidebar groups sessions by the same rule: a project's group is exactly
  the sessions its deck opens.

## Agent control

The session's project is the assistant's to manage, like its own deck: its
cards and its workspaces.

Configuration is in the `@rumu` workspace and is changed with the file
tools. What changes the branch (mounting a deck, the default workspace)
stays a tool. Which set a workspace is in shows in its path, not in a field:

```
@rumu:/workspaces/<name>      every visible workspace (the registry)
@rumu:/project/workspaces     the session's project's mounted workspaces
```

- `list @rumu:/workspaces` lists the registry: each workspace with its
  location, marked when it is mounted in the session's project or is the
  default. `read @rumu:/workspaces/<name>` returns its entry, `write`
  creates one:

  ```yaml
  root: ~/src/tokio
  ssh: build@ci.example   # optional: a remote workspace
  read_only: true         # optional
  ```

  A local `root` must be an existing directory. Changing an existing entry
  asks the user (`workspace`), since other projects may mount it.
- `@rumu:/project/workspaces` is an ordered YAML list of registered names,
  the first being the project's default. Writing it mounts, unmounts and
  reorders. Like a card, it must be read before it is written, so two
  sessions of the project cannot overwrite each other's change. It exists
  only in a session with a project.
- Mounting is a property of the project, not of the workspace, so it is not
  kept under `@rumu:/workspaces`. The registry listing only shows it.
- `workspace_use(name?)` sets the branch's default and lists the workspaces,
  the project's first. `name` may be any registered workspace, `@scratch`, or
  `@rumu`.
- `card_mount(deck, at: "project")` moves the session into a project, and
  `card_mount(at: "project")` leaves it. `card_mount()` marks project decks
  in its listing.
- `card_mount(create: <name>, at: "project")` creates a project deck with a
  blank charter and moves the session into it (`create` is accepted only
  there). The assistant then writes the charter and mounts workspaces.

| what                                          | approval            |
|-----------------------------------------------|---------------------|
| register a workspace                          | none                |
| change a registered workspace                 | `workspace`         |
| mount, unmount or reorder the project's ones  | none                |
| write the session's project deck              | none                |
| leave the project                             | none                |
| move into an existing project                 | `mount` of its deck |
| create a project                              | `create`            |

- `create` and `workspace` are approval actions like those in
  [Approvals](decks.md#approvals). "Always allow" is stored by target:
  `{ "workspace": { <assistant deck id>: [<workspace name>, …] } }`, and
  `create`, which has none, with `*`.
- Deleting a workspace and mounting workspaces in another project are the
  user's, in the web UI.

## Web UI

- The sidebar is a two-level tree: the projects and one group for sessions
  without a project, each with its sessions, newest first. Projects are
  ordered by latest activity. A session row names its assistant unless it is
  the default one.
- Each project row has actions to start a session in it and to open its
  settings. Dragging a session to another group moves it: the same marker as
  `card_mount`, with no approval.
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
- Several projects in one session.
- An `ask` session in the asking session's project.
