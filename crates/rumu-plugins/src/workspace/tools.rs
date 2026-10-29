//! The model-facing tools: schemas, `<ws>:<path>` target resolution and the
//! implementations over the workspace backends. The `@rumu`
//! virtual workspace (`/card`, `/session`) is routed from here into
//! [`cards`] (and, for `/session`, `session`).

use std::time::Duration;

use rumu_cards::usage::CardOp;
use rumu_cards::{card, library};
use rumu_core::{
    Effect, Manifest, PathState, Progress, RUMU_WORKSPACE, ToolError, ToolOutput, ToolResult,
    ToolResultExt, ToolSpec, Utf8TypedPathBuf, Utf8UnixPath, Utf8UnixPathBuf, Workspace,
    WorkspaceUse, parse_args, resolve_state, tool_schema,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::backend::{BackendError, ExecOutput, probe_uname};
use super::{WorkspacePlugin, usable_ctl_dir};
use crate::Result;
use rumu_core::Cx;

/// A malformed tool argument (surfaces as an error `ToolOutput`).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub(super) struct ToolInputError(String);

impl From<String> for ToolInputError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for ToolInputError {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// A finished `bash` call: exit status, combined output and the effects it
/// produced. `is_error` follows the exit status.
struct BashOutcome {
    tool: String,
    code: i32,
    output: String,
    effects: Vec<Effect>,
}

impl From<BashOutcome> for ToolOutput {
    fn from(outcome: BashOutcome) -> Self {
        ToolOutput {
            content: rumu_core::Content::text(format!(
                "[{}] exit code: {}\n{}",
                outcome.tool, outcome.code, outcome.output
            )),
            is_error: outcome.code != 0,
            effects: outcome.effects,
        }
    }
}

const BASH_TIMEOUT_DEFAULT: u64 = 60;
const BASH_TIMEOUT_MAX: u64 = 600;
pub(super) const LIST_LIMIT: usize = 100;

const INSTRUCTIONS: &str = "Files live in workspaces — one project each, on this machine or a \
remote one over ssh. Every file tool takes one `path`: a path in the default workspace (relative \
to its root, or absolute on its machine), or `<workspace>:<path>` for another workspace, e.g. \
`proj:src/main.rs`, `app:/var/log/app.log`. `workspace_use(name)` changes the default; \
`workspace_use()` lists workspaces. `bash` runs in a workspace root, so relative paths mean the \
same thing in bash and in the file tools. Read-only workspaces refuse writes and bash.\n\n\
The `@rumu` workspace holds rumu's own data and has no bash. `@rumu:/card/<id>` are knowledge \
cards: `read` returns an editable view (YAML frontmatter, then the body), and writes are refused \
if the card changed since you last read it — re-read and merge. \
`@rumu:/session/<session>/<node>` and `@rumu:/session/<session>/<from>..<to>` are past \
conversation records (read-only); `search(query, scope: \"@rumu:/session\")` searches them. If \
the user rewinds the conversation, card edits persist.";

// ---------------------------------------------------------------------------
// Tool arguments (the single source of truth for parsing and schemas)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// file path (workspace-qualified, or relative to the default workspace's root)
    path: String,
    /// 1-based start line
    offset: Option<u64>,
    /// number of lines
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// file path (workspace-qualified, or relative to the default workspace's root)
    path: String,
    /// full file content (for cards: the editable view — frontmatter + body)
    content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    /// file path (workspace-qualified, or relative to the default workspace's root)
    path: String,
    /// exact text to replace
    old_string: String,
    /// replacement
    new_string: String,
    /// replace every occurrence
    replace_all: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListArgs {
    /// directory (workspace-qualified, or relative to the default workspace's root)
    path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    /// shell snippet
    command: String,
    /// workspace name; default: the branch default
    workspace: Option<String>,
    /// seconds, default 60, max 600
    timeout: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// search terms
    query: String,
    /// `@rumu:/card/<prefix>` or `@rumu:/session[/<session>]`
    scope: Option<String>,
    /// max hits (default 10)
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceUseArgs {
    /// workspace name (a preset or `@rumu`)
    name: Option<String>,
}

/// The plugin manifest.
pub(super) fn manifest() -> Manifest {
    Manifest {
        name: "workspace".into(),
        instructions: Some(INSTRUCTIONS.into()),
        tools: vec![
            ToolSpec {
                name: "read".into(),
                description: "Read a file's raw text content (no line numbers). `path` is a \
workspace path (relative to the workspace root, absolute on its machine, or \
`<workspace>:<path>`; `@rumu:/card/<id>` for a card). `offset` is a 1-based start line and \
`limit` a line count; a trailing `#L<n>` / `#L<a>-L<b>` anchor sets them when omitted."
                    .into(),
                schema: tool_schema::<ReadArgs>(),
                background: false,
            },
            ToolSpec {
                name: "write".into(),
                description: "Create or overwrite a file (parent directories are created; \
rejected on read-only workspaces). Same `path` addressing as `read`; for cards the \
content is the editable view — frontmatter + body."
                    .into(),
                schema: tool_schema::<WriteArgs>(),
                background: false,
            },
            ToolSpec {
                name: "edit".into(),
                description: "Replace an exact string in a file. Errors on 0 matches \
and on multiple matches unless `replace_all`; rejected on read-only \
workspaces."
                    .into(),
                schema: tool_schema::<EditArgs>(),
                background: false,
            },
            ToolSpec {
                name: "list".into(),
                description: "List a directory's entries, one per line (directories \
get a trailing `/`). Omitted `path` = the default workspace's root; `proj:` / \
`@rumu:` (nothing after the colon) = that workspace's root. On the `@rumu` \
workspace, lists cards."
                    .into(),
                schema: tool_schema::<ListArgs>(),
                background: false,
            },
            ToolSpec {
                name: "bash".into(),
                description: "Run a command with `bash -c` in a workspace's root \
directory (`workspace` argument; default = the default workspace). Returns combined \
stdout+stderr (truncated to ~20k chars) and the exit code. Default timeout 60s (max \
600s). Disabled on read-only workspaces and on the `@rumu` workspace."
                    .into(),
                schema: tool_schema::<BashArgs>(),
                background: true,
            },
            ToolSpec {
                name: "search".into(),
                description: "Search cards (default) or past conversations by text. Both are \
relevance-ranked and match any term. `scope` narrows it: `@rumu:/card/<prefix>` for a card \
directory, `@rumu:/session` for all past conversations, `@rumu:/session/<session>` for one. \
Returns addresses with short snippets, never full text — follow up with `read`."
                    .into(),
                schema: tool_schema::<SearchArgs>(),
                background: false,
            },
            ToolSpec {
                name: "workspace_use".into(),
                description: "Set the default workspace for subsequent tool calls \
(branch state; forks inherit it) and return the workspace listing. Without \
`name`, just list workspaces. `name` may be a preset or `@rumu`."
                    .into(),
                schema: tool_schema::<WorkspaceUseArgs>(),
                background: false,
            },
        ],
        hooks: vec![],
    }
}

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

/// Where a tool call lands: a real workspace preset (with the absolute path
/// on its machine), or the `@rumu` virtual workspace (with its normalized
/// `/card/…` / `/session/…` path).
enum Target {
    Real {
        ws: Workspace,
        abs: Utf8TypedPathBuf,
    },
    Rumu {
        abs: Utf8UnixPathBuf,
    },
}

/// `[{prefix}] {message}` — the prefix every tool/workspace failure carries.
pub(super) fn prefixed(prefix: &str, message: impl std::fmt::Display) -> String {
    format!("[{prefix}] {message}")
}

/// A backend operation failure: `[{tool}] {op} {path}: {source}`, or
/// `[{tool}] {op}: {source}` without a path.
fn backend_err(
    tool: &str,
    op: &str,
    path: Option<String>,
    source: impl std::fmt::Display,
) -> ToolError {
    let at = path.map(|path| format!(" {path}")).unwrap_or_default();
    ToolError::model(prefixed(tool, format!("{op}{at}: {source}")))
}

/// The read-only refusal shared by the write/edit/bash tools.
pub(super) fn read_only_err(name: &str, ws: &Workspace, action: &str) -> ToolError {
    ToolError::model(prefixed(
        name,
        format!("workspace `{}` is read-only; {action}", ws.name),
    ))
}

/// Reject a malformed card id before it becomes a path.
pub(super) fn invalid_card_id(id: &str) -> Option<ToolError> {
    (!rumu_cards::valid_card_id(id)).then(|| {
        ToolError::model(prefixed(
            RUMU_WORKSPACE,
            format!(
                "invalid card id `{id}`: use `/`-separated segments of \
                 [A-Za-z0-9._-], no leading dots, max depth 8"
            ),
        ))
    })
}

/// `[@rumu] no card `<uri>``, plus near matches when there are any.
pub(super) fn no_card_err(id: &str, known: &[String]) -> ToolError {
    let mut message = format!("no card `{}`", rumu_cards::card_uri(id));
    let suggestions = rumu_cards::suggest_ids(known, id);
    if !suggestions.is_empty() {
        message.push_str(&format!(" (did you mean: {}?)", suggestions.join(", ")));
    }
    ToolError::model(prefixed(RUMU_WORKSPACE, message))
}

/// `[@rumu] loading card `<uri>`: {source}`.
pub(super) fn card_load_err(id: &str, source: impl std::fmt::Display) -> ToolError {
    ToolError::model(prefixed(
        RUMU_WORKSPACE,
        format!("loading card `{}`: {source}", rumu_cards::card_uri(id)),
    ))
}

/// `[@rumu] unknown path ...`.
pub(super) fn unknown_rumu_path(abs: &Utf8UnixPath) -> ToolError {
    ToolError::model(prefixed(
        RUMU_WORKSPACE,
        format!("unknown path `{RUMU_WORKSPACE}:{abs}` (roots: /card, /session, /task)"),
    ))
}

/// The `search` scope refusal.
fn bad_scope() -> ToolError {
    ToolError::model(format!(
        "`scope` must address the `{RUMU_WORKSPACE}` workspace: \
         `{RUMU_WORKSPACE}:/card/<prefix>` or `{RUMU_WORKSPACE}:/session[/<session>]`"
    ))
}

/// Resolve a `path` argument into a [`Target`].
fn target_of(state: &PathState, raw: &str) -> ToolResult<Target> {
    let addr = resolve_state(state, raw).map_err(ToolError::model)?;
    if addr.ws == RUMU_WORKSPACE {
        let abs = Utf8UnixPathBuf::try_from(addr.abs).expect("the @rumu workspace is always Unix");
        return Ok(Target::Rumu { abs });
    }
    let ws = state
        .workspaces
        .iter()
        .find(|w| w.name == addr.ws)
        .cloned()
        .expect("resolve only returns known workspaces");
    Ok(Target::Real { ws, abs: addr.abs })
}

/// Resolve `bash`'s `workspace` argument (a bare name) or the branch
/// default.
fn bash_target(state: &PathState, raw: Option<&str>) -> ToolResult<Workspace> {
    let raw = raw.or(state.default_workspace.as_deref()).ok_or_else(|| {
        ToolError::model(
            "no default workspace is set — call `workspace_use()` to list workspaces and pick one",
        )
    })?;
    if raw == RUMU_WORKSPACE {
        return Err(ToolError::model(
            "the `@rumu` workspace has no bash; pass `workspace`",
        ));
    }
    state
        .workspaces
        .iter()
        .find(|w| w.name == raw)
        .cloned()
        .ok_or_else(|| ToolError::model(unknown_workspace(state, raw)))
}

/// The unknown-workspace error, listing every addressable name.
fn unknown_workspace(state: &PathState, name: &str) -> String {
    let mut names: Vec<&str> = state.workspaces.iter().map(|w| w.name.as_str()).collect();
    names.push(RUMU_WORKSPACE);
    format!(
        "unknown workspace `{name}` (available: {})",
        names.join(", ")
    )
}

/// Slice raw text by 1-based `offset` line / `limit` line count.
pub(super) fn slice_lines(
    text: String,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<String, ToolInputError> {
    if let Some(0) = offset {
        return Err("`offset` is 1-based".into());
    }
    if let Some(0) = limit {
        return Err("`limit` must be at least 1".into());
    }
    if offset.is_none() && limit.is_none() {
        return Ok(text);
    }
    let lines = text
        .lines()
        .skip(offset.unwrap_or(1).saturating_sub(1) as usize);
    let picked: Vec<&str> = match limit {
        Some(n) => lines.take(n as usize).collect(),
        None => lines.collect(),
    };
    Ok(picked.join("\n"))
}

/// Count-limited exact string replacement for `edit`.
fn replace_occurrences(
    text: &str,
    old: &str,
    new: &str,
    all: bool,
) -> Result<(String, usize), ToolInputError> {
    if old.is_empty() {
        return Err("`old_string` must not be empty".into());
    }
    let count = text.matches(old).count();
    match count {
        0 => Err("`old_string` not found".into()),
        1 => Ok((text.replacen(old, new, 1), 1)),
        n if !all => Err(format!(
            "`old_string` matches {n} times; pass `replace_all: true` to replace all"
        )
        .into()),
        n => Ok((text.replace(old, new), n)),
    }
}

/// Apply a `#L<n>` / `#L<a>-L<b>` anchor to the offset/limit the call
/// did not already carry.
fn apply_anchor(
    raw: &str,
    mut offset: Option<u64>,
    mut limit: Option<u64>,
) -> ToolResult<(String, Option<u64>, Option<u64>)> {
    let (path, anchor) = rumu_core::split_line_anchor(raw);
    if let Some((a, b)) = anchor {
        if offset.is_none() {
            offset = Some(a);
        }
        if let Some(b) = b
            && limit.is_none()
        {
            if b < a {
                return Err(ToolError::model(format!(
                    "`{raw}`: the anchor's end line is before its start line"
                )));
            }
            limit = Some(b - a + 1);
        }
    }
    Ok((path.to_owned(), offset, limit))
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// The `Plugin::call_tool` dispatch (kept here so the tool methods stay
/// private to this module). Each tool closes its result (and applies the
/// output cap) with [`ToolResultExt::finish`].
pub(super) async fn call(
    p: &WorkspacePlugin,
    name: &str,
    args: Value,
    cx: &Cx,
) -> rumu_core::Result<ToolOutput> {
    let state = cx.state()?;
    let state = state.as_ref();
    match name {
        "read" => p.tool_read(state, &args, cx).await.finish(),
        "write" => p.tool_write(state, &args, cx).await.finish(),
        "edit" => p.tool_edit(state, &args, cx).await.finish(),
        "list" => p.tool_list(state, &args, cx).await.finish(),
        "bash" => p.tool_bash(state, &args, cx).await.finish(),
        "workspace_use" => p.tool_use(state, &args, cx).await.finish(),
        "search" => p.tool_search(state, &args, cx).await.finish(),
        _ => Ok(ToolOutput::err(format!(
            "workspace plugin has no tool `{name}`"
        ))),
    }
}
impl WorkspacePlugin {
    async fn tool_read(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: ReadArgs = parse_args("read", args)?;
        let (path, offset, limit) = apply_anchor(&args.path, args.offset, args.limit)?;
        let target = target_of(state, &path)?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                let name = ws.name.clone();
                let backend = self.backend_for(&ws);
                match backend.read_bytes(&abs).await {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        match slice_lines(text, offset, limit) {
                            Ok(s) => Ok(ToolOutput::ok(s).with_effects(effects)),
                            Err(e) => {
                                Err(ToolError::model(prefixed(&name, e)).with_effects(effects))
                            }
                        }
                    }
                    Err(e) => Err(backend_err(&name, "read", Some(display(&ws, &abs)), e)
                        .with_effects(effects)),
                }
            }
            Target::Rumu { abs } => self.rumu_read(&abs, offset, limit, cx).await,
        }
    }

    /// Shared write preflight for real workspaces: the read-only check.
    fn write_target(&self, ws: &Workspace) -> ToolResult<String> {
        if ws.read_only {
            return Err(read_only_err(&ws.name, ws, "writes are refused"));
        }
        Ok(ws.name.clone())
    }

    async fn tool_write(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: WriteArgs = parse_args("write", args)?;
        let target = target_of(state, &args.path)?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                let name = match self.write_target(&ws) {
                    Ok(n) => n,
                    Err(e) => return Err(e.with_effects(effects)),
                };
                let backend = self.backend_for(&ws);
                match backend.write_bytes(&abs, args.content.as_bytes()).await {
                    Ok(()) => Ok(ToolOutput::ok(format!(
                        "[{name}] wrote {} ({} bytes)",
                        display(&ws, &abs),
                        args.content.len()
                    ))
                    .with_effects(effects)),
                    Err(e) => Err(backend_err(&name, "write", Some(display(&ws, &abs)), e)
                        .with_effects(effects)),
                }
            }
            Target::Rumu { abs } => {
                let Some(id) = card_id_of(&abs) else {
                    return Err(self.rumu_write_refused(&abs));
                };
                self.card_write(id, &args.content, CardOp::Write, cx).await
            }
        }
    }

    async fn tool_edit(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: EditArgs = parse_args("edit", args)?;
        let target = target_of(state, &args.path)?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                let name = match self.write_target(&ws) {
                    Ok(n) => n,
                    Err(e) => return Err(e.with_effects(effects)),
                };
                let backend = self.backend_for(&ws);
                let text = match backend.read_bytes(&abs).await {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => text,
                        Err(_) => {
                            return Err(ToolError::model(prefixed(
                                &name,
                                format!(
                                    "edit {}: not a UTF-8 text file; use bash",
                                    display(&ws, &abs)
                                ),
                            ))
                            .with_effects(effects));
                        }
                    },
                    Err(e) => {
                        return Err(backend_err(&name, "edit", Some(display(&ws, &abs)), e)
                            .with_effects(effects));
                    }
                };
                let (new_text, n) = match replace_occurrences(
                    &text,
                    &args.old_string,
                    &args.new_string,
                    args.replace_all.unwrap_or(false),
                ) {
                    Ok(x) => x,
                    Err(e) => {
                        return Err(ToolError::model(prefixed(&name, e)).with_effects(effects));
                    }
                };
                match backend.write_bytes(&abs, new_text.as_bytes()).await {
                    Ok(()) => Ok(ToolOutput::ok(format!(
                        "[{name}] edited {} ({n} replacement{})",
                        display(&ws, &abs),
                        if n == 1 { "" } else { "s" }
                    ))
                    .with_effects(effects)),
                    Err(e) => Err(backend_err(&name, "edit", Some(display(&ws, &abs)), e)
                        .with_effects(effects)),
                }
            }
            Target::Rumu { abs } => {
                // Edit = string replace on the view, then the write path. The
                // read-freshness check comes first: a stale read is the more
                // fundamental problem than a non-matching string.
                let Some(id) = card_id_of(&abs) else {
                    return Err(self.rumu_write_refused(&abs));
                };
                let existing = match library::load_card(&self.card_store(), id) {
                    Ok(Some(c)) => c,
                    Ok(None) => return Err(no_card_err(id, &[])),
                    Err(e) => return Err(card_load_err(id, e)),
                };
                if let Some(err) = self.ensure_fresh_read(id, &existing, cx) {
                    return Err(err);
                }
                let view = card::view_of(&existing);
                let (new_view, n) = match replace_occurrences(
                    &view,
                    &args.old_string,
                    &args.new_string,
                    args.replace_all.unwrap_or(false),
                ) {
                    Ok(x) => x,
                    Err(e) => return Err(ToolError::model(prefixed(RUMU_WORKSPACE, e))),
                };
                let out = self.card_write(id, &new_view, CardOp::Edit, cx).await?;
                if out.is_error {
                    return Ok(out);
                }
                Ok(ToolOutput::ok(format!(
                    "[{RUMU_WORKSPACE}] edited card `{}` ({n} replacement{})",
                    rumu_cards::card_uri(id),
                    if n == 1 { "" } else { "s" }
                ))
                .with_effects(out.effects))
            }
        }
    }

    async fn tool_list(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        // Omitted path = the default workspace's root; `proj:` / `@rumu:` —
        // nothing after the colon — is that workspace's root too.
        let args: ListArgs = parse_args("list", args)?;
        let target = target_of(state, args.path.as_deref().unwrap_or(""))?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                let name = ws.name.clone();
                let backend = self.backend_for(&ws);
                match backend.list_dir(&abs).await {
                    Ok(entries) => {
                        let lines: Vec<String> = entries
                            .into_iter()
                            .map(|(n, dir)| if dir { format!("{n}/") } else { n })
                            .collect();
                        let body = if lines.is_empty() {
                            "(empty)".to_owned()
                        } else {
                            lines.join("\n")
                        };
                        Ok(ToolOutput::ok(format!("[{name}] {body}")).with_effects(effects))
                    }
                    Err(e) => Err(backend_err(&name, "list", Some(display(&ws, &abs)), e)
                        .with_effects(effects)),
                }
            }
            Target::Rumu { abs } => self.rumu_list(&abs, cx).await,
        }
    }

    async fn tool_bash(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<BashOutcome> {
        let args: BashArgs = parse_args("bash", args)?;
        let timeout = args
            .timeout
            .unwrap_or(BASH_TIMEOUT_DEFAULT)
            .clamp(1, BASH_TIMEOUT_MAX);
        let ws = bash_target(state, args.workspace.as_deref())?;
        if ws.read_only {
            return Err(read_only_err(&ws.name, &ws, "bash is disabled"));
        }
        let effects = self.maybe_inject_agents(cx, &ws).await;
        let name = ws.name.clone();
        let backend = self.backend_for(&ws);
        // Stream every chunk to the UI (tool.progress) / the task buffer
        // (task.progress). Background bash has no timeout of its own: it ends
        // when the command exits, `task_kill`s it, or its owner aborts.
        let mut on_chunk = |chunk: &str| cx.progress(Progress::Append(chunk.to_owned()));
        let exec: std::result::Result<ExecOutput, BackendError> =
            if cx.call().is_some_and(|c| c.background) {
                backend.exec(&args.command, &mut on_chunk).await
            } else {
                match tokio::time::timeout(
                    Duration::from_secs(timeout),
                    backend.exec(&args.command, &mut on_chunk),
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        return Err(ToolError::model(format!(
                            "[{name}] `{}` timed out after {timeout}s",
                            args.command
                        ))
                        .with_effects(effects));
                    }
                }
            };
        match exec {
            Ok(out) => Ok(BashOutcome {
                tool: name,
                code: out.code,
                output: out.combined,
                effects,
            }),
            Err(e) => Err(backend_err(&name, "bash", None, e).with_effects(effects)),
        }
    }

    async fn tool_use(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: WorkspaceUseArgs = parse_args("workspace_use", args)?;
        let Some(name) = args.name else {
            // Listing only: no marker is written.
            return Ok(self.listing(state).await.into());
        };
        if name != RUMU_WORKSPACE && !state.workspaces.iter().any(|w| w.name == name) {
            return Err(ToolError::model(unknown_workspace(state, &name)));
        }
        let mut effects = Vec::new();
        if let Some(ws) = state.workspaces.iter().find(|w| w.name == name).cloned() {
            effects.extend(self.maybe_inject_agents(cx, &ws).await);
        }
        effects.push(Effect::marker(&WorkspaceUse { name: name.clone() }));
        // The marker lands at the next safe point; report the listing as if
        // the new default were already in effect.
        let mut after = state.clone();
        after.default_workspace = Some(name.clone());
        Ok(ToolOutput::ok(format!(
            "[{name}] default workspace set; workspaces:\n{}",
            self.listing(&after).await
        ))
        .with_effects(effects))
    }

    /// One line per workspace: the default marker `*`, name, machine
    /// label (`local` / `ssh:prod`), root, rw/ro (+ the remote OS/arch for
    /// ssh once probed), then the `@rumu` line.
    async fn listing(&self, state: &PathState) -> String {
        let mut lines = Vec::new();
        if state.workspaces.is_empty() {
            lines.push(
                "no workspaces configured (presets live in RUMU_HOME/config.yaml)".to_owned(),
            );
        }
        for ws in &state.workspaces {
            let star = if state.default_workspace.as_deref() == Some(ws.name.as_str()) {
                "* "
            } else {
                "  "
            };
            let rw = if ws.read_only { "ro" } else { "rw" };
            let mach = ws.label();
            let mut line = format!("{star}{} {mach} {} {rw}", ws.name, ws.root);
            if let Some(host) = &ws.ssh {
                let ctl_dir = usable_ctl_dir(&self.ssh_ctl_dir);
                if let Some(uname) = probe_uname(host, ctl_dir.as_deref(), &self.ssh_uname).await {
                    line.push_str(&format!(" {uname}"));
                }
            }
            lines.push(line);
        }
        let star = if state.default_workspace.as_deref() == Some(RUMU_WORKSPACE) {
            "* "
        } else {
            "  "
        };
        lines.push(format!(
            "{star}{RUMU_WORKSPACE} rumu's own data: {RUMU_WORKSPACE}:/card (knowledge cards), \
             {RUMU_WORKSPACE}:/session (past conversations); no bash"
        ));
        lines.join("\n")
    }
}

/// The canonical display form of a real-workspace path.
fn display(ws: &Workspace, abs: &Utf8TypedPathBuf) -> String {
    rumu_core::display(&ws.name, ws.root.to_path(), abs.to_path())
}

/// The card id a `/card/<id>[.md]` path addresses, if it is one.
pub(super) fn card_id_of(abs: &Utf8UnixPath) -> Option<&str> {
    let id = abs.strip_prefix("/card").ok()?.as_str();
    (!id.is_empty()).then(|| id.strip_suffix(".md").unwrap_or(id))
}

// ---------------------------------------------------------------------------
// the `@rumu` virtual filesystem
// ---------------------------------------------------------------------------

impl WorkspacePlugin {
    /// `read` inside the `@rumu` workspace. Rendered text is sliced by
    /// `offset`/`limit`; the tool boundary caps the model-facing text.
    async fn rumu_read(
        &self,
        abs: &Utf8UnixPath,
        offset: Option<u64>,
        limit: Option<u64>,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if let Some(id) = card_id_of(abs) {
            return self.card_read(id, offset, limit, cx).await;
        }
        if abs.starts_with("/session") {
            return self.session_read(abs, offset, limit, cx).await;
        }
        if abs.starts_with("/task") {
            return self.task_read(abs, offset, limit, cx).await;
        }
        Err(unknown_rumu_path(abs))
    }

    /// The refusal for writes outside `/card/<id>`: `/session` is
    /// read-only, everything else is unknown.
    fn rumu_write_refused(&self, abs: &Utf8UnixPath) -> ToolError {
        if abs.starts_with("/session") {
            ToolError::model("`@rumu:/session` is read-only")
        } else {
            unknown_rumu_path(abs)
        }
    }

    /// `list` inside the `@rumu` workspace.
    async fn rumu_list(&self, abs: &Utf8UnixPath, cx: &Cx) -> ToolResult<ToolOutput> {
        match abs.as_str() {
            "/" => Ok(format!("[{RUMU_WORKSPACE}] card/\nsession/\ntask/").into()),
            "/card" => self.card_list("").await,
            _ if abs.starts_with("/card") => {
                let prefix = abs
                    .strip_prefix("/card")
                    .map(Utf8UnixPath::as_str)
                    .unwrap_or("");
                self.card_list(prefix).await
            }
            _ if abs.starts_with("/session") => self.session_list(abs, cx).await,
            _ if abs.starts_with("/task") => self.task_read(abs, None, None, cx).await,
            _ => Err(unknown_rumu_path(abs)),
        }
    }

    /// The `search` tool: cards by default, past conversations with
    /// `scope: "@rumu:/session[/<session>]"`. Read-only — no `card_use`
    /// markers.
    async fn tool_search(
        &self,
        state: &PathState,
        args: &Value,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let args: SearchArgs = parse_args("search", args)?;
        let limit = args.limit.unwrap_or(10).max(1) as usize;
        let scope = args.scope.as_deref().unwrap_or("");
        // An omitted scope means the whole card library; a given one is a
        // qualified path prefix that must land in the `@rumu` workspace.
        let abs: Utf8TypedPathBuf = if scope.is_empty() {
            Utf8TypedPathBuf::from_unix("/card")
        } else {
            match rumu_core::resolve_state(state, scope) {
                Ok(addr) if addr.ws == RUMU_WORKSPACE => addr.abs,
                _ => return Err(bad_scope()),
            }
        };
        let Utf8TypedPathBuf::Unix(abs) = abs else {
            return Err(bad_scope());
        };
        if abs.starts_with("/session") {
            let sid = abs
                .strip_prefix("/session")
                .map(Utf8UnixPath::as_str)
                .unwrap_or("");
            return self.session_search(&args.query, sid, limit, cx).await;
        }
        if abs.starts_with("/card") {
            let prefix = abs
                .strip_prefix("/card")
                .map(Utf8UnixPath::as_str)
                .unwrap_or("");
            return rumu_cards::search_text(
                &self.card_store(),
                &args.query,
                &json!({ "prefix": prefix, "limit": limit }),
            );
        }
        Err(bad_scope())
    }
}
