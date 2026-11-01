---
keywords: [rumu, architecture, crates, agent]
description: Overview of the rumu agent framework, crate layout, and core invariants.
scope: [scratch]
---

rumu is the agent framework this demo server belongs to. Layout:

- `crates/rumu-core` — shared types + traits (Plugin, Host, Node, Event, Action).
- `crates/rumu-kernel` — agent loop, hook dispatch, predicate engine, projection.
- `crates/rumu-plugins` — built-in plugins (memory, workspace, code) + stdio host.
- `web/` — Nuxt 4 frontend; talks to the server over one WebSocket.

Core ideas: a session is a tree of events; the context is the projection of one
root→head path; workspaces/hooks/memory are all plugins.
