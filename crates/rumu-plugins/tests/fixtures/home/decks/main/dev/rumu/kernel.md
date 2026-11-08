---
description: The agent loop, hook dispatch and projection in crates/rumu-kernel.
---
`crates/rumu-kernel` is the only "business" code in rumu:

- **Agent loop** (`turn.rs`): per step — flush queued notes, dispatch
  `llm.pre`, project the path, call the model, append the Assistant node,
  run tool calls sequentially, flush again at the safe point. A sub-agent
  ends when a reply carries no tool calls; its summary comes back as the
  `agent` tool's result with a transcript address.
- **Hook dispatch** (`dispatch.rs`): two phases so guards always judge the
  final payload; intercept hooks are awaited with a timeout and fail closed.
- **Projection**: nodes → messages deterministically; system + tools are
  frozen per session for the cache contract.

Related: [The rumu project](../../rumu).
