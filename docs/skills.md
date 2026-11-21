# Skills

A skill is a directory with a `SKILL.md`, in the
[Agent Skills](https://agentskills.io/specification) format: YAML frontmatter
with a `name` and a `description` (what it does and when to use it), then
instructions, and any files beside it (`scripts/`, `references/`, …). rumu
finds the skills on its host, an assistant enables some of them, and the
model reads one when a task matches its description.

## Prior art

Read from the Agent Skills specification and its guide for clients.

| Question | Common answer | Taken here |
| --- | --- | --- |
| Where skills are found | User-level `~/.<client>/skills`, `~/.agents/skills`; project-level `.agents/skills` | rumu's installed repositories, `~/.agents/skills`, `~/.claude/skills`; not project-level yet |
| What the model sees first | Each skill's name and description, in the system prompt or an activation tool's description | A first-call note |
| Loading one | The file-read tool on its `SKILL.md`, or an `activate_skill` tool | `read @rumu:/skills/<name>` |
| Name collisions | Deterministic precedence, the shadowed one reported | First found wins; Settings marks the others |
| Malformed frontmatter | Lenient: load when possible, skip without a description | The same |

## Where skills are found

- In precedence order:
  1. Repositories installed from git, under `RUMU_HOME/skills/<repo>`: each
     directory with a `SKILL.md`, at most three levels down (a repository
     can be one skill or hold many, as under `skills/`).
  2. `~/.agents/skills` and then `~/.claude/skills` on the rumu host, at
     most two levels down. rumu only reads these: other tools install into
     them.
- Every lookup scans again, so a skill added or edited on disk is seen at
  once; nothing is cached or recorded.
- Lenient, as skills are written for other harnesses: a missing `name` is
  the directory's; a frontmatter that is not YAML is read line by line (an
  unquoted value with `: `). A skill without a description is unusable, and
  so is a name that cannot be used in paths and rules (1–64 letters, digits
  and `-_.`, not starting with `.`).
- Settings › Skills lists every skill found, with its directory, why it is
  unusable, or that it is shadowed by an earlier one of the same name.

## Installed repositories

- Settings › Skills installs a repository from a git URL, under a name (by
  default `owner-repo` from the URL, without `.git`). rumu runs
  `git clone --depth 1`, never prompting, so a private repository needs
  credentials git already has.
- The clone is the record: Settings shows its `origin`, pulls it
  (fast-forward only) and deletes it. Nothing is written to `settings.yaml`.

## Assistants

- An assistant's `deck.yaml` has the rules enabling skills, set on the
  assistant page:

  ```yaml
  skills: ["*", "!caveman"]   # all but this one, new skills included
  ```

  The rules work like a provider's model rules ([MCP](mcp.md) tools use
  them too): `*` matches any run of characters, `!` excludes, and a rule
  naming a skill wins over a pattern. None enables nothing.
- A session uses its assistant's rules as `deck.yaml` has them now, and
  the skills are read live, so changing either reaches its running sessions
  at their next model call. An `ask` session uses the asked assistant's.
- A rule naming a skill that is no longer found stays in `deck.yaml` and
  shows as unavailable on the assistant page.

## The model's view

- The first model call of a path whose session enables any usable skill
  gets a note: how to use skills, then each one's name and description.
  When the set of enabled skills changes, the next model call gets the list
  again, or a note that none is left.
- `@rumu:/skills` holds the session's skills:
  - `list @rumu:/skills`: each with its description.
  - `read @rumu:/skills/<name>`: its `SKILL.md`, then a list of its other
    files.
  - `read @rumu:/skills/<name>/<path>`: one of its files; a binary one is
    refused with its path on the rumu host. `list` lists a directory.
- A skill's scripts run where the model runs commands: in its workspace.
  The model writes a script there before running it; rumu does not copy
  skills to workspaces.
- Skills outside the session's rules are neither listed nor readable.

## Not yet

- Project-level skills (`.agents/skills` in a workspace). The model can
  find and read them itself.
- Activating a skill from the composer (`/name`).
- Keeping a read skill through compaction: after one, the note lists the
  skills again and the model reads one again when it needs it.
