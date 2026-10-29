# rumu

A small, calm AI agent with a good memory. Built in Rust.

rumu keeps what it learns as knowledge cards it can recall across sessions,
runs on a small Rust core, and lives in a quiet, carefully made web interface.

## Run

```sh
cargo build
(cd web && pnpm install && pnpm build)

# Anthropic-compatible endpoint; token is sent as x-api-key
export ANTHROPIC_BASE_URL=... ANTHROPIC_AUTH_TOKEN=...
RUMU_MODEL=your-model-id ./target/debug/rumu
# → http://127.0.0.1:7878 (RUMU_PORT overrides the port, RUMU_HOST the bind address)
# RUMU_EFFORT sets the reasoning effort: low, medium, high (default), xhigh, max.
# RUST_LOG overrides the default `info` log level (logs go to stderr).
```

`bash` runs unsandboxed on the machine hosting the server, so the server is
meant to be local-only: it binds `127.0.0.1` by default and rejects
cross-origin WebSocket upgrades.

The release build embeds `web/dist`: run `pnpm build` before
`cargo build --release`. Debug builds read `web/dist` from disk, so a
`pnpm build` is picked up without recompiling.
Frontend dev: `cd web && pnpm dev` (Nuxt on 5173, proxies `/ws` to 7878), or
`pnpm mock` to run the real server from the repo root with a scripted LLM
(`RUMU_HOME=web/mock/home RUMU_LLM=script:web/mock/demo.yaml`). No model
endpoint is needed: `web/mock/demo.yaml` cycles through scripted replies that
exercise streaming text and reasoning, card reads and search, `bash`, `code`, a
sub-agent, and a background task. `RUMU_LLM=script:<path>` selects this mode.

The web's wire types (`web/app/lib/types.ts`) re-export TypeScript generated
from the Rust serde types by ts-rs. After changing a wire type, run
`cargo test` to regenerate `web/app/lib/generated`; those files are checked in
because the web build imports them.

`RUMU_HOME` (default `~/.rumu`) holds `config.yaml` (workspace presets and
external plugins), `cards/` and `sessions/`. With no config at all,
the current directory becomes the default workspace, so it runs out of the box.

Workspaces are a named map. Each entry is a root string or a map with `root`
plus optional `ssh` (a remote `[user@]host`) and `read_only`:

```yaml
workspaces:
  proj: ~/code/proj
  app:
    ssh: deploy@prod
    root: /srv/app
    read_only: true
```

rumu's own data — knowledge cards and past conversations — lives in the
reserved `@rumu:/…` workspace. The leading `@` is not a valid preset name, so
a project called `rumu` can be an ordinary workspace.

## Layout

```
crates/rumu-core      shared types + traits (Plugin, Cx, Node, Event, Decision)
crates/rumu-cards     card content model, storage, discovery and usage
crates/rumu-kernel    agent loop, hook dispatch, predicate engine, sub-agents
crates/rumu-store     per-session JSONL files
crates/rumu-llm       genai adapter: projection + cache breakpoints
crates/rumu-plugins   memory / workspace / code + stdio plugin host
crates/rumu-server    `rumu` binary (axum, /ws)
web/                  Nuxt 4 (SPA) + Tailwind v4 frontend
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT License](LICENSE-MIT), at your option.
