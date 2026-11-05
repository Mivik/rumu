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
external plugins), `decks/` (cards, assistants and their history), `grants.json`
(remembered mount / ask permissions) and `sessions/`. With no config at all,
the current directory becomes the default workspace, so it runs out of the
box.

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

A **deck** is a set of cards about one subject, kept as a bare git repository
at `RUMU_HOME/decks/<name>/`; see [docs/decks.md](docs/decks.md). Its identity
and defaults live in `deck.yaml`. rumu is the only writer: every card write is
a commit, and decks are not edited by hand. Decks are the
unit of memory, permission and history. A session mounts decks under
`@rumu:/cards/<mount>/` (its own deck at `/cards/self`) and sees only those
cards and its own past sessions. An **assistant** is a deck with
`assistant: true`: its pinned cards are its persona and instructions, and its
`deck.yaml` names the decks it mounts by default.

```yaml
# deck.yaml of the deck at RUMU_HOME/decks/ledger
id: 0193a2f1-7c4e-7b2a-9d01-5e8f3a6b2c10
name: ledger
assistant: true
workspace: proj
mounts:
  - { deck: <finance-deck-id>, at: /cards/finance, write: true }
```

Decks are always referred to by id; the name is for display. Which
assistant is the default is per user, in `RUMU_HOME/default-assistant`. A
session starts with its own deck plus these default mounts; `card_mount`
changes them for the branch. Decks are created by the user (the web UI or by
hand), never by an assistant; assistants write cards in the decks they mount
with `write`. Mount, ask and link approvals marked "always allow" are kept in
`RUMU_HOME/grants.json`. History uses one git repository per deck through
gitoxide (`gix`); no git binary is needed at runtime.

rumu's own data — knowledge cards and past conversations — lives in the
reserved `@rumu:/…` workspace. The leading `@` is not a valid preset name, so
a project called `rumu` can be an ordinary workspace.

## Images

Prompts (and tool results) can carry images. Uploads go to
`POST /blobs?name=<file name>` (raw body = image bytes) and come back as an
`Attachment`; the web client then sends `{hash, name}` references with a
`prompt` and serves them back from `GET /blobs/{hash}`. The bytes live in
`<RUMU_HOME>/blobs`, one file per SHA-256, normalized once at ingest
(orientation applied, long edge ≤ 2000 px, PNG or JPEG out) so what the model
sees never changes afterwards — the projection, and so the prompt cache, is
byte-stable. In text, an `[Image #N]` anchor marks where the N-th attachment
sits (`what is [Image #1]?`); attachments without an anchor follow the text.
The server garbage-collects blobs no session references, with a 24 h grace
period that covers the gap between an upload and the node that references it.

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
