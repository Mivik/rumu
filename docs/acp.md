# ACP

`rumu acp` speaks the [Agent Client Protocol](https://agentclientprotocol.com)
(protocol v1) on stdin/stdout, so an editor (Zed, JetBrains) or an evaluation
harness (Harbor) can run rumu as its agent. The process boots the whole rumu
stack over `RUMU_HOME`, like the server does; the two cannot share a home at
once (the second one refuses to start). Logs go to stderr.

## Sessions

- `session/new` takes the client's `cwd` as a workspace: a registered local
  workspace with that root is reused, otherwise one is registered, named
  after the directory. The session goes in the first project that mounts
  the workspace, or in a new project of the same name, and the workspace is
  its default.
- An ACP session is the main line of one rumu session; it shows in the web
  UI like any other. Regenerating, editing, side questions and other
  branches have no place in ACP and are not offered.
- Not supported: `session/load`, `session/resume`, `session/list`, MCP
  servers passed by the client, the client's file system and terminal (rumu
  runs its own tools), images in prompts.

## Prompts

- A `session/prompt` answers once the main line is idle: no turn running or
  about to start, and no background sub-agent still at work. A background
  tool task (`bash` with `background: true`) does not hold it, since it may
  never end (a dev server); when it finishes, the turn it wakes comes out as
  `session/update`s with no prompt waiting.
- The stop reason is `end_turn`, `cancelled` after `session/cancel`, and
  `refusal` when a hook blocked the prompt. A failed turn is an error
  response carrying its message. The response's `usage` sums the tokens of
  every model call the prompt led to, sub-agents included.
- `session/cancel` aborts the main line and its background work, and every
  prompt sent or queued before it ends as `cancelled`.
- A second `session/prompt` while one runs waits for it, then runs as a
  turn of its own (`agentCapabilities._meta.promptQueueing`).
- `_session/steering` (`{sessionId, prompt, _meta?}`, announced in
  `InitializeResponse._meta.steering.supported`) puts a message into the
  running turn (`injected`). With no prompt running it starts a turn no
  prompt waits for (`startedNewTurn`), or, when the request asks with
  `_meta.steering.idleBehavior: "promptRequired"`, does nothing
  (`promptRequired`).

## Updates

- Reply and reasoning stream as `agent_message_chunk` /
  `agent_thought_chunk`.
- Each tool call is a `tool_call`, with `kind` `read`, `edit` (`write`,
  `edit`), `delete` (`delete`), `execute` (`bash`), `search` (`search`,
  `list`) or `other`, and the local file it is about as its location. Output
  a running tool prints updates its content (at most once a second, the last
  8 KiB); its result completes or fails it. A `write`/`edit`/`delete` result
  carries its change as diffs: one per hunk (with context), or the whole new
  file (a deleted file's diff empties it).
- A sub-agent is its `agent` tool call; the branch it runs on is in that
  call's `_meta.rumu.branch`. Its own steps are not shown.

## Questions

- An approval rumu asks for (mounting or writing another deck, handing a
  task to another assistant, creating a project, changing a workspace) is a
  `session/request_permission` with `allow_once`, `allow_always` and
  `reject_once`. A cancelled request or an option rumu does not know is a
  refusal.
- `ask` becomes an `elicitation/create` form when the client declares
  `elicitation.form`: per question a choice (one or several) plus a
  free-text field. Without form support each question is answered "no user
  can answer here", so the model goes on by itself.

## Config options

`session/new` returns three select options, set with
`session/set_config_option`:

- `assistant`: the assistant deck the session runs as. It can change only
  before the first prompt (the session is replaced).
- `model` (category `model`): `default` (the assistant's model) or a model
  ref (`provider/model`, aliases as `rumu/<name>`) from Settings.
- `effort` (category `thought_level`): `default` or a reasoning effort.

## Zed

In Zed's `settings.json`:

```json
{
  "agent_servers": {
    "rumu": {
      "type": "custom",
      "command": "rumu",
      "args": ["acp"],
      "env": {}
    }
  }
}
```

Stop a running `rumu` server on the same home first, or give the agent its
own one with `"env": { "RUMU_HOME": "/path/to/another/home" }`.

## Harbor

Harbor's generic ACP agent runs `rumu acp` inside each task container.
`scripts/harbor/` holds what it needs:

1. `scripts/harbor/build.sh` builds a static (musl) binary into
   `target/harbor/`, with `launch.sh`, which starts every run on a fresh
   `RUMU_HOME` seeded from `/opt/rumu/home`.
2. Put a `settings.yaml` with the provider to use in `target/harbor/home/`.
   Leave MCP servers out. A fresh home's assistant has no model of its own,
   so name one with `-m`: a model ref from those settings, or an alias as
   `rumu/<name>`.
3. Mount the directory at `/opt/rumu` and point Harbor at
   `scripts/harbor/agent.json`:

   ```sh
   harbor run -p <tasks> -a acp -m <model> \
     --ak registry_entry_path=scripts/harbor/agent.json \
     --mounts '[{"type":"bind","source":"'"$PWD"'/target/harbor","target":"/opt/rumu","read_only":true}]' \
     -n 4
   ```

Harbor's runner installs the Python `agent-client-protocol` unpinned, and
1.0.0 (2026-11-09) no longer has what it imports (harbor#2206 is the same
problem with an earlier release). Until Harbor pins it, add
`--ae UV_EXCLUDE_NEWER=2026-11-08T00:00:00Z` so the runner gets 0.9.0; the
variable reaches the agent's environment too, which matters only for tasks
that install Python packages with uv.
