# Plugins

An external plugin is a program rumu runs and talks to over its stdin and
stdout. It can add tools, hook events (and stop a tool call), add text to the
system prompt, and draw on the page: results by their data tag, labels above
the composer, slash commands, side panels, and a settings form. Plugins are
installed by the user and trusted as such; their web side runs in rumu's page.

The TypeScript SDK, `@rumu/plugin`, is a separate repository (rumu-sdk). This
page is the protocol it speaks; a plugin in any language may speak it too.

## Registering

A plugin is a named entry in `RUMU_HOME/settings.yaml`, under `plugins`,
edited in Settings → Plugins (rumu is the file's only writer):

```yaml
plugins:
  scratch-path:                   # the name: 1–64 of [A-Za-z0-9_-]
    command: [python3, plugins/scratch-path.py]
    cwd: plugins                  # optional, relative to RUMU_HOME
    settings: { sides: 20 }       # its settings form's values
```

- An entry that fails to start, or a process that exits, takes only itself
  out: it shows as failed with its error and the tail of its stderr, and
  restarts from Settings.
- Editing an entry restarts it; a change of `settings` alone reaches the
  running plugin as a `settings` notification.
- When a file its command names (past the program) changes, the plugin
  restarts, and when its web entry changes, the page loads it anew: a
  `vite build --watch` reloads a plugin under development.
- Before settings.yaml, plugins were listed in `RUMU_HOME/config.yaml`
  (`plugins: [{ name, cmd, cwd }]`). A start that finds one moves its
  entries into settings.yaml and keeps the old file as `config.yaml.migrated`.

## Protocol

Newline-delimited JSON-RPC 2.0. The types named here are exported from the
Rust ones with ts-rs (`web/app/lib/generated`).

rumu → plugin:

| Method | Params | Result |
| --- | --- | --- |
| `initialize` | `{ host: "rumu", version, settings }` | a `Manifest` (`name`, `instructions`, `tools`, `hooks`), and optionally `rumu`, `web`, `settings` (below) |
| `tool/call` | `{ name, args, ctx, session, branch, background }` | a `ToolOutput` |
| `hook` | `{ hook_id, event: Event, ctx }` | a `HookOutput` |
| `call` | `{ method, params, branch }` — the web side's `ctx.call` | any JSON |
| `settings` (notification) | the settings values | |
| `shutdown` (notification) | | |

`initialize`'s extra fields:

- `rumu`: the rumu versions the plugin works with, a semver requirement
  (`^0.1`). One this rumu does not meet fails the start, saying so.
- `web`: the plugin's web entry, an ES module, as a path (relative to its
  `cwd`). rumu serves it and the files beside it at `/plugins/<name>/…`.
- `settings`: a JSON Schema object; each of its `properties` (`string`,
  `integer`, `number`, `boolean`, with `title`, `default`, `enum`, `minimum`,
  `maximum`) is a row of its form in Settings.

plugin → rumu, any time a request of rumu's is in flight (`ctx` is the token
that request carried):

| Method | Params | Result |
| --- | --- | --- |
| `host/path` | `{ ctx }` | the branch's nodes as the model sees them |
| `host/state` | `{ ctx }` | its `PathState` |
| `host/index` | `{ ctx, tag }` | the payloads on its path with that tag |
| `host/progress` | `{ ctx, append? , data? }` | live output of a tool call |

and at any time:

| Method | Params | Result |
| --- | --- | --- |
| `host/post` | `{ branch, effects, wake? }` (a `Post`) | notes on a branch |
| `host/emit` | `{ event, data?, branch? \| ctx? }` | an event for the plugin's web side, on the pages showing that branch's session (every page without one) |

## Web side

The page imports a running plugin's web entry and calls its default export
with a context:

- `renderResult(tag, component)`: tool results and notes whose data carries
  `tag` are drawn by `component` (props `data`, `v`, `text`). The first plugin
  to claim a tag keeps it. The text stays what the model, ACP and chat apps
  read; on the page it is the hover title.
- `addStatus({ id, render, title?, onClick? })`: a label above the composer.
- `addCommand({ name, run })`: `/name arg` in the composer runs it instead of
  sending.
- `addPanel({ id, title, component })` and `openPanel(id)`: a sheet beside the
  chat, where a sub-agent or an artifact opens.
- `call(method, params)`, `on(event, fn)`: the backend, both ways.
- `settings`, `session` (`id`, `branch`, `send(text)`), and `experimental`
  (a few of rumu's components; not promised).

It shares the page's Vue, found at `globalThis.__rumu.vue` (the SDK's Vite
preset points `vue` there). Each thing a plugin registers is mounted inside
an error boundary: a plugin that throws breaks only its own part, which says
so, and a setup that throws is a warning.

A plugin does not replace rumu's own parts (tool rows, the composer) and
cannot override a built-in tool.
