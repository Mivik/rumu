# rumu

A small, calm AI agent with a good memory. Built in Rust.

rumu keeps what it learns as knowledge cards it can recall across sessions,
runs on a small Rust core, and lives in a quiet, carefully made web interface.

## Run

```sh
cargo build
(cd web && pnpm install && pnpm build)

./target/debug/rumu
# → http://127.0.0.1:7878 (RUMU_PORT / RUMU_HOST override; RUST_LOG sets the log level)
```

On first start the server prints a link with an access token
(`http://127.0.0.1:7878/#token=…`); open it once and the browser remembers it.
Everything else — providers and models, assistants, decks, workspaces,
projects, MCP servers — is set up in the web UI. Data lives in `RUMU_HOME`
(default `~/.rumu`).

`bash` runs unsandboxed on the server's machine, and anyone with the token has
full access. The server binds `127.0.0.1` by default; for remote access use an
SSH tunnel or HTTPS through a reverse proxy.

## Concepts

- **Deck** — a set of cards about one subject, versioned as a git repository;
  the unit of memory, permission and history. See [docs/decks.md](docs/decks.md).
- **Assistant** — a deck whose pinned cards are its persona and instructions.
- **Workspace** — a named directory, local or over ssh, addressed as
  `<name>:<path>`. Each session also gets a throwaway `@scratch`.
- **Project** — what a session works on: a deck plus the workspaces it mounts.
  See [docs/projects.md](docs/projects.md).

More: [compaction](docs/compaction.md), [MCP](docs/mcp.md), [skills](docs/skills.md),
[editors and Harbor over ACP](docs/acp.md).

## Development

- `cd web && pnpm dev` — Nuxt dev server on 5173, proxying to the server on 7878.
- `cd web && pnpm mock` — the real server with a scripted LLM
  (`web/mock/demo.yaml`), no model endpoint needed.
- Debug builds read `web/dist` from disk; release builds embed it, so run
  `pnpm build` before `cargo build --release`.
- Wire types are generated from Rust by ts-rs: after changing one, run
  `cargo test` to regenerate `web/app/lib/generated` (checked in).

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
