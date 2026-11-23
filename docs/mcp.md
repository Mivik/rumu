# MCP

rumu is an MCP client. The user registers MCP servers in Settings, and an
assistant uses the servers bound to it. Their tools are not in the model's
tool list: each server is an object of the `@rumu` workspace whose actions are
its tools, which the model finds there and runs with `invoke`.

## Prior art

Read from the Harness Lens comparison of 37 open-source harnesses and the
harnesses' own docs.

| Question | Common answer | Taken here |
| --- | --- | --- |
| Where servers are defined | Globally (Codex, OpenCode, Cline); some also inline per agent (Claude Code, Gemini CLI, Copilot) | Globally, per user; an assistant names the ones it uses |
| Many tools | Deferred: names first, schemas on demand (Claude Code `ToolSearch`, OpenAI `tool_search`) | Always deferred, through `@rumu:/mcp` |
| Loading a found tool | Provider-native: the schema is injected into the conversation and the tool called by name | Not portable across providers, so the generic `invoke` |
| Tool naming | `mcp__<server>__<tool>` (Claude Code), `<server>_<tool>` (OpenCode) | `<server>/<tool>`, the action's full name |
| Approval | Per tool or per server, varies | None: binding a server to an assistant authorizes it |
| Narrowing a server's tools | Allow and deny lists per server (Codex `enabled_tools`/`disabled_tools`) | Rules per binding, as for models |

## Servers

- A server is a named entry in `RUMU_HOME/settings.yaml`, under `mcp`,
  edited in Settings. rumu is the file's only writer; it holds secrets, so it
  is written owner-only.

  ```yaml
  mcp:
    github:                       # the name: 1–64 of [A-Za-z0-9_-]
      command: [github-mcp-server, stdio]
      env: { GITHUB_TOKEN: … }    # added to rumu's environment
    linear:
      url: https://mcp.example/mcp   # streamable HTTP
      headers: { Authorization: Bearer … }
  ```

  An entry has either `command` (stdio, run in `RUMU_HOME`) or `url`
  (streamable HTTP). The name is fixed once created: decks refer to it.
- The web never sees the values of `env` and `headers`, only their names.
  An edit sends a value to replace it, and omits it to keep it.
- rumu connects every server at startup and whenever its entry changes, in
  the background. Each server is `connecting`, `ready` (with its tools) or
  `failed` (with the error). Settings shows the status and the tools, and
  can reconnect a server.
- The tool list is refreshed when the server sends `tools/list_changed`.
  Resources are not cached: listing a server asks it each time. A request
  to a server whose connection has dropped reconnects once. Resource
  subscriptions are not supported yet.
- OAuth is not supported yet: a server that needs it takes a token in
  `headers`.

## Assistants

- An assistant's `deck.yaml` names the servers it uses, each with the rules
  enabling its tools, set on the assistant page:

  ```yaml
  mcp:
    github: ["*", "!delete_*"]   # all but these, new tools included
    linear: [list_issues]        # only this one
  ```

  The rules work like a provider's model rules: `*` matches any run of
  characters, `!` excludes, and a rule naming a tool wins over a pattern.
  Binding a server enables `*`; its tool count on the assistant page opens
  a checklist of the tools the server lists now. Unchecking under `*` adds
  `!tool`, so tools the server adds later are in; checking from none adds
  the tool's name, so they are out.

- A session uses its assistant's servers as `deck.yaml` has them now, so
  changing the assistant reaches its running sessions at their next model
  call. An `ask` session uses the asked assistant's.
- A name that is not (or no longer) in Settings stays in `deck.yaml` and
  shows as unavailable, on the assistant page and to the model.
- Calls need no approval. Like a `deck.yaml` mount, binding a server is the
  user's decision that the assistant may use it.

## The model's view

- The first model call of a path whose session has servers gets a note
  listing them: name, status, tool count, and the server's own
  `instructions` (capped). When the assistant's set of servers changes, the
  next model call gets the list again, or a note that none is left.
- `@rumu:/mcp` holds the session's servers. A server is an object and its
  tools are its actions, named `<server>/<tool>` (a tool is not a path):
  - `list @rumu:/mcp`: what it holds, then each server with its status and
    tool count.
  - `list @rumu:/mcp/<server>`: its status; its resources (`uri — title:
    first line of the description (mime)`) and resource templates, when it
    declares `resources`, the first 100 of each and a count of the rest;
    then its tools under `actions:`, one line each: the signature
    (`fake/echo(text: string)`) and the first line of its description,
    and last the path's own action `mcp.read_resource`.
  - `read @rumu:/mcp/<server>`: its `instructions`, then each tool's
    description and input schema, then its resources.
  - `search` with `scope: "@rumu:/mcp[/<server>]"`: BM25 over tool names,
    titles and descriptions. Each hit is an `action` (`action fake/echo`)
    and prints its schema, so the model can run it right away; the default
    limit is 5.
- `invoke { action: "<server>/<tool>", args }` runs it (no `path`: the
  name says the server). The result's text and images come back as the tool
  result, `isError` as an error; each image is also copied to
  `@scratch:.inbox/attachments/` like a prompt's, the text naming the file.
  An unknown name is refused with the server's tool names, a call the server
  refuses with the tool's input schema. It can run in the background, and
  `code` scripts reach it as `rumu.invoke`.
- Other media in a result (audio, binary embedded resources) are copied to
  `@scratch:.inbox/attachments/` as they are, with an extension from their
  MIME type (`bin` when it is not a plain one), the text naming the file;
  without a `@scratch` they are only named. Embedded text resources come
  back as their text. A `resource_link` comes back as its URI and how to
  read it.
- `invoke { action: "mcp.read_resource", path: "@rumu:/mcp/<server>",
  args: { uri } }` reads a resource (`resources/read`): a listed one, one
  from a template, or a link from a tool result. Its contents come back like
  a tool result's embedded resources, several each under its URI. Reading
  needs no approval either, and the tool rules do not narrow it.
- Servers outside the session's list, and tools its rules leave out, are
  neither listed nor callable; a server's tool count is of the enabled ones.
