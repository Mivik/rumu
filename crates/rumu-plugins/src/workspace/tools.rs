//! The model-facing tools: schemas, `<ws>:<path>` target resolution and the
//! implementations over the workspace backends. Paths in the `@rumu`
//! virtual workspace are routed to their nodes by [`super::vfs`].

use rumu_cards::Cards;
use rumu_core::{
    Artifact, ArtifactKind, Effect, INVOKE_OUTPUT_LIMIT, Manifest, PathState, Progress,
    RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, ToolError, ToolOutput, ToolResult,
    ToolResultExt, ToolSpec, Utf8TypedPathBuf, Utf8UnixPathBuf, Workspace, WorkspaceUse,
    parse_args, resolve_state, tool_schema, truncate_middle,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::backend::{Backend, BackendError, ExecOutput, is_not_found, probe_uname, sh_quote};
use super::patch;
use super::vfs::bad_scope;
use super::watch::WatchArgs;
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
/// produced. `is_error` follows the exit status. `structured` is set for a
/// plugin invoke (a `code` script), which then gets a [`BashResult`].
struct BashOutcome {
    code: i32,
    output: String,
    effects: Vec<Effect>,
    structured: bool,
}

/// What a `code` script's `rumu.bash()` resolves to (also for a non-zero
/// exit code). The `output` keeps its start and end within
/// [`INVOKE_OUTPUT_LIMIT`].
#[derive(Debug, serde::Serialize, Deserialize, JsonSchema)]
struct BashResult {
    exit_code: i32,
    /// combined stdout+stderr
    output: String,
    /// whether the middle of `output` was cut
    truncated: bool,
}

impl Structured for BashResult {
    const TAG: &'static str = "bash.result";
    const V: u32 = 1;
}

/// Keep the last `n` lines of `text` (all of it with no `n`), behind a
/// marker counting the dropped ones. A trailing newline ends the last line
/// rather than starting an empty one.
fn last_lines(text: String, n: Option<usize>) -> String {
    let Some(n) = n else { return text };
    let body = text.strip_suffix('\n').unwrap_or(&text);
    let total = body.split('\n').count();
    if total <= n {
        return text;
    }
    let start = match n {
        0 => text.len(),
        n => body
            .rmatch_indices('\n')
            .nth(n - 1)
            .map_or(0, |(i, _)| i + 1),
    };
    format!(
        "… [{} earlier lines omitted] …\n{}",
        total - n,
        &text[start..]
    )
}

impl From<BashOutcome> for ToolOutput {
    fn from(outcome: BashOutcome) -> Self {
        let mut content =
            rumu_core::Content::text(format!("exit code: {}\n{}", outcome.code, outcome.output));
        if outcome.structured {
            let truncated = outcome.output.chars().count() > INVOKE_OUTPUT_LIMIT;
            content = content.with_data(&BashResult {
                exit_code: outcome.code,
                output: truncate_middle(outcome.output, INVOKE_OUTPUT_LIMIT),
                truncated,
            });
        }
        ToolOutput {
            content,
            is_error: outcome.code != 0,
            effects: outcome.effects,
        }
    }
}

pub(super) const LIST_LIMIT: usize = 100;
/// The largest text file `show` snapshots (it renders in the page).
const ARTIFACT_MAX_BYTES: usize = 5 * 1024 * 1024;
/// The largest other file `show` snapshots (media, downloads).
const ARTIFACT_MAX_BINARY_BYTES: usize = 64 * 1024 * 1024;

const INSTRUCTIONS: &str = "Files live in workspaces. A workspace is a name for a root \
directory on this machine or an ssh host; the name is not a folder and never appears inside a \
path. A tool `path` is either a plain path in the default workspace (relative to its root, or \
absolute on its machine), or `<workspace>:<path>` for any workspace — the colon separates the \
name from the path:\n\
- `src/main.rs`, `/etc/hosts` → the default workspace\n\
- `app:/var/log/app.log` → workspace `app`\n\
- `@scratch:page.html`, `@scratch:` (its root) → the scratch workspace\n\
Name files the same way when you mention them to the user. \
`bash` runs in the default workspace's root (or `workspace`'s), so relative paths match the file \
tools. `list @rumu:/workspaces` lists the workspaces; `invoke` `workspace.use` on one \
(`path: \"@rumu:/workspaces/<name>\"`) makes it the default. Read-only \
workspaces refuse writes and bash.\n\n\
`@scratch` is this session's throwaway workspace on this machine, shared with its branches and \
sub-agents and deleted with the session. Use it for scripts, downloads, clones and generated \
files; what should outlive the session belongs in a card or a project workspace. Its \
`.inbox/` is written by rumu (attachments, events): read it, don't change it. In `bash` on this \
machine, its directory is `$RUMU_SCRATCH_DIR`.\n\n\
To give the user a page, document, diagram, code, image, audio or video to look at, or any file \
to download, write it to a file (usually in \
`@scratch`) and `show` it; `show` it again after a change for a new version. To put an image in \
your reply itself, \
write `![alt](<address>)` with the address of one already in the conversation: a `show`n file, \
or an attachment's `@scratch:.inbox/…` copy; any other address shows as a link. For something \
interactive in the reply itself (a chart, a slider, a small calculator), write a ```widget \
fence holding an HTML fragment: the web client runs it in place, sandboxed, as wide as the reply \
and as tall as its content; the user can save it as a file. The user sees the widget, not its \
source. Its plain HTML is already styled like the app (text, headings, tables, buttons, inputs, \
sliders, progress bars; `button.primary`; classes `card`, `row`, `grid`, `stat` with a `<b>` \
number, `muted`), so add only the layout you need and use the app's colors as CSS variables \
(`--ink`, `--ink-2`, `--ink-3`, `--line`, `--surface`, `--surface-2`, `--brand`; series colors \
`--c1`…`--c6`); Chart.js charts take them by default. Both a widget and a `show`n HTML page run \
sandboxed: scripts and styles may load from \
cdn.jsdelivr.net, cdnjs.cloudflare.com and unpkg.com, nothing else from the network (inline \
images); `rumu.prompt(text)` puts text in the user's message box for them to send.\n\n\
`@rumu` holds rumu's own data as paths (no bash): `list @rumu:/` shows what is there, and \
`list` on any of its paths says what it holds, what may be written and what asks the user, and \
the actions `invoke` runs on it. Its cards (`@rumu:/cards/<mount>/<path>`) are your memory: \
`read` gives an editable view (YAML frontmatter, then the body); a write is refused if the card \
changed since your read — re-read and merge. Card edits survive a rewind.";

// ---------------------------------------------------------------------------
// Tool arguments (the single source of truth for parsing and schemas)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// `<workspace>:<path>`, or a path in the default workspace
    path: String,
    /// 1-based start line
    offset: Option<u64>,
    /// number of lines
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    /// `<workspace>:<path>`, or a path in the default workspace
    path: String,
    /// full file content (for cards: the editable view — frontmatter + body)
    content: String,
    /// for cards: move the card at this path to `path` (a write plus removal;
    /// nothing carries over)
    from: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    /// `<workspace>:<path>`, or a path in the default workspace
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
    /// directory: `<workspace>:<path>`, or a path in the default workspace
    path: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    /// `<workspace>:<path>`, or a path in the default workspace
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BashArgs {
    /// shell snippet
    command: String,
    /// the call's intent, 5-10 words in active voice ("List files in src",
    /// "Run the parser tests"); shown as the call's summary in the transcript
    description: Option<String>,
    /// workspace name; default: the branch default
    workspace: Option<String>,
    /// seconds, default 60, max 600: a call still running then moves to the
    /// background (in `code`: is stopped); with `watch`: its lifetime,
    /// default 1800, max 86400
    timeout: Option<u64>,
    /// return only the last N lines of output; the user still sees all of it
    /// live, and the exit code stays the command's (unlike `| tail`)
    tail: Option<usize>,
    /// with `background: true`: send new output lines to you as notes while
    /// it runs (`{}` for every line)
    watch: Option<WatchArgs>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ShowArgs {
    /// `<workspace>:<path>`, or a path in the default workspace
    path: String,
    /// display title; default: the file name
    title: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// search terms
    query: String,
    /// `@rumu:/cards/<prefix>`, `@rumu:/sessions[/<session>]` or `@rumu:/mcp[/<server>]`
    scope: Option<String>,
    /// max hits (default 10; 5 for tools)
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InvokeArgs {
    /// the action's full name: `<type>.<action>` for one of rumu's
    /// (`task.kill`), `<server>/<tool>` for an MCP tool
    action: String,
    /// the `@rumu` object it acts on (`@rumu:/tasks/<id>`); none for an MCP
    /// tool
    path: Option<String>,
    /// its arguments, per its schema
    args: Option<serde_json::Map<String, Value>>,
}

/// The plugin manifest.
pub(super) fn manifest() -> Manifest {
    Manifest {
        name: "workspace".into(),
        instructions: Some(INSTRUCTIONS.into()),
        tools: vec![
            ToolSpec {
                name: "read".into(),
                description: "Read a file's raw text content (no line numbers), or view an \
image (PNG, JPEG, GIF, WebP) as an image attachment. `path`: a plain path (default workspace) or \
`<workspace>:<path>`. `offset` is a 1-based start line and `limit` a line count (text only — \
they do not apply to images); a trailing `#L<n>` / `#L<a>-L<b>` anchor sets them when omitted."
                    .into(),
                schema: tool_schema::<ReadArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "write".into(),
                description: "Create or overwrite a file (its directory must already exist; \
rejected on read-only workspaces). Same `path` addressing as `read`. In `@rumu`, `list` a path \
for what may be written there; `from` moves a card or renames a deck."
                    .into(),
                schema: tool_schema::<WriteArgs>(),
                background: false,
                output_schema: None,
                parallel: false,
            },
            ToolSpec {
                name: "edit".into(),
                description: "Replace an exact string in a file. Errors on 0 matches \
and on multiple matches unless `replace_all`; rejected on read-only \
workspaces."
                    .into(),
                schema: tool_schema::<EditArgs>(),
                background: false,
                output_schema: None,
                parallel: false,
            },
            ToolSpec {
                name: "delete".into(),
                description: "Delete a file (not a directory: use `bash` for those; \
rejected on read-only workspaces), or what `@rumu` lets you delete (a card, after reading it). \
Same `path` addressing as `read`; a rewind restores a deleted file."
                    .into(),
                schema: tool_schema::<DeleteArgs>(),
                background: false,
                output_schema: None,
                parallel: false,
            },
            ToolSpec {
                name: "list".into(),
                description: "List a directory's entries, one per line (directories \
get a trailing `/`). Omitted `path` = the default workspace's root; `proj:` / \
`@rumu:` (nothing after the colon) = that workspace's root. In `@rumu`, it also says what a \
path holds, then its `files:` and `actions:`."
                    .into(),
                schema: tool_schema::<ListArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "bash".into(),
                description: "Run a command with `bash -c` in a workspace's root \
directory (`workspace` argument; default = the default workspace). Returns combined \
stdout+stderr (truncated to ~20k chars) and the exit code. A call still running after \
`timeout` (default 60s, max 600s) moves to the background. Disabled on read-only \
workspaces and on the `@rumu` workspace. Give every call a \
`description` (5-10 words, active voice): the transcript shows it as the call's summary. \
`watch` (with `background: true`) delivers new output lines as notes that wake you, until the command exits, `until` matches, `task.kill` \
or its lifetime ends. Prefer `match`/`until` \
to piping into grep (pipes buffer lines), and make `match` cover failures too (Traceback, \
Error, Killed): silence is not success. Use `tail`, not `| tail`, to keep only the end of \
the output."
                    .into(),
                schema: tool_schema::<BashArgs>(),
                background: true,
                output_schema: Some(tool_schema::<BashResult>()),
                parallel: false,
            },
            ToolSpec {
                name: "show".into(),
                description: "Show a file to the user as an artifact: a snapshot rendered \
beside the chat by its extension (`.html` runs sandboxed, network only to script CDNs; `.md`, `.svg`; images, \
audio, video and PDF display or play; other text as highlighted code; anything else as a \
download). Same `path` addressing as `read`; not on `@rumu`. Showing the same path again adds a \
version."
                    .into(),
                schema: tool_schema::<ShowArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "search".into(),
                description: "Search by text, relevance-ranked (any term matches): the \
mounted decks' cards by default, or the `@rumu` subtree `scope` names — past conversations \
(`@rumu:/sessions[/<session>]`) or MCP tools (`@rumu:/mcp[/<server>]`). Each hit is marked \
`card`, `session` or `action`; cards and conversations come as addresses with short snippets \
(`read` them), tools with their input schemas."
                    .into(),
                schema: tool_schema::<SearchArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "invoke".into(),
                description: "Run an action on an `@rumu` object: `action` is its full name \
and `path` the object (`task.kill` on `@rumu:/tasks/<id>`); `list` an object for its actions \
and their arguments. An MCP tool is an action of its server: `action` is `<server>/<tool>`, \
with no `path`."
                    .into(),
                schema: tool_schema::<InvokeArgs>(),
                background: true,
                output_schema: None,
                parallel: false,
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
/// `/cards/…` / `/sessions/…` path).
enum Target {
    Real {
        ws: Workspace,
        abs: Utf8TypedPathBuf,
    },
    Rumu {
        abs: Utf8UnixPathBuf,
    },
}

/// A backend operation failure: `{op} {path}: {source}`, or
/// `{op}: {source}` without a path.
fn backend_err(op: &str, path: Option<String>, source: impl std::fmt::Display) -> ToolError {
    let at = path.map(|path| format!(" {path}")).unwrap_or_default();
    ToolError::model(format!("{op}{at}: {source}"))
}

/// The read-only refusal shared by the write/edit/bash tools.
pub(super) fn read_only_err(ws: &Workspace, action: &str) -> ToolError {
    ToolError::model(format!("workspace `{}` is read-only; {action}", ws.name))
}

/// Reject a card reference that is not under a mounted deck.
pub(super) fn invalid_card_ref(cards: &Cards, public: &str) -> Option<ToolError> {
    if cards.split(public).is_some() {
        return None;
    }
    if cards.mount_of(public).is_some() {
        return Some(ToolError::model(format!(
            "invalid card path `{}`: name a card below the mount point, with segments of \
             `[A-Za-z0-9._-]` that do not start with `.`",
            rumu_cards::card_uri(public)
        )));
    }
    let mounts: Vec<String> = cards
        .mounts()
        .iter()
        .map(|m| format!("`{}/`", m.point()))
        .collect();
    Some(ToolError::model(format!(
        "card `{}` is not under a mounted deck ({})",
        rumu_cards::card_uri(public),
        if mounts.is_empty() {
            "nothing is mounted; `invoke deck.mount` on a `@rumu:/decks/<name>` mounts one"
                .to_owned()
        } else {
            mounts.join(", ")
        }
    )))
}

/// `no card `<uri>``, plus near matches when there are any.
pub(super) fn no_card_err(public: &str, known: &[String]) -> ToolError {
    let mut message = format!("no card `{}`", rumu_cards::card_uri(public));
    let suggestions = rumu_cards::suggest_ids(known, public);
    if !suggestions.is_empty() {
        message.push_str(&format!(" (did you mean: {}?)", suggestions.join(", ")));
    }
    ToolError::model(message)
}

/// `loading card `<uri>`: {source}`.
pub(super) fn card_load_err(public: &str, source: impl std::fmt::Display) -> ToolError {
    ToolError::model(format!(
        "loading card `{}`: {source}",
        rumu_cards::card_uri(public)
    ))
}

/// What `show` records: a workspace file's bytes put in the blob store,
/// titled by its file name. Errors are messages about the file.
async fn snapshot(
    backend: &dyn Backend,
    blobs: rumu_blob::Result<rumu_blob::Blobs>,
    ws: &Workspace,
    abs: &Utf8TypedPathBuf,
) -> std::result::Result<Artifact, String> {
    let at = display(ws, abs);
    let bytes = backend.read_bytes(abs).await.map_err(|e| e.to_string())?;
    let name = abs.file_name().unwrap_or(at.as_str()).to_owned();
    let kind = ArtifactKind::of(&name, std::str::from_utf8(&bytes).is_ok());
    let limit = match kind {
        ArtifactKind::Html | ArtifactKind::Markdown | ArtifactKind::Svg | ArtifactKind::Code => {
            ARTIFACT_MAX_BYTES
        }
        _ => ARTIFACT_MAX_BINARY_BYTES,
    };
    if bytes.len() > limit {
        return Err(format!(
            "{} bytes is over the {limit}-byte limit",
            bytes.len()
        ));
    }
    let hash = blobs
        .and_then(|blobs| blobs.put(&bytes))
        .map_err(|e| e.to_string())?;
    Ok(Artifact {
        kind,
        mime: rumu_core::media_type(&name).map(str::to_owned),
        path: at,
        title: name,
        hash,
        size: bytes.len() as u64,
    })
}

/// [`snapshot`] of an address on a branch's path, outside any tool call: the
/// web opens a file no `show` recorded this way. Nothing is recorded.
pub async fn snapshot_file(
    state: &PathState,
    raw: &str,
    blobs: rumu_blob::Blobs,
) -> std::result::Result<Artifact, String> {
    let (ws, abs) = real_file(state, raw)?;
    snapshot(
        &*super::backend_for(&super::ssh_ctl_dir(), &ws),
        Ok(blobs),
        &ws,
        &abs,
    )
    .await
}

/// A page the user saved from a reply's widget, written into the branch's
/// `@scratch` under its hash and snapshotted the way `show` would, so it
/// opens as an artifact and stays a file the model can work on. Nothing is
/// recorded.
pub async fn save_page(
    state: &PathState,
    page: &str,
    blobs: rumu_blob::Blobs,
) -> std::result::Result<Artifact, String> {
    let hash = rumu_blob::hash_of(page.as_bytes());
    let raw = format!(
        "{}:widget-{}.html",
        rumu_core::SCRATCH_WORKSPACE,
        &hash[..12]
    );
    let (ws, abs) = real_file(state, &raw)?;
    let backend = super::backend_for(&super::ssh_ctl_dir(), &ws);
    backend
        .write_bytes(&abs, page.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    snapshot(&*backend, Ok(blobs), &ws, &abs).await
}

/// A workspace file's address, resolved; errors are messages about it.
fn real_file(
    state: &PathState,
    raw: &str,
) -> std::result::Result<(Workspace, Utf8TypedPathBuf), String> {
    let target = target_of(state, raw).map_err(|e| match e {
        ToolError::Model(out) => out.content.text,
        ToolError::Fatal(e) => e.to_string(),
    })?;
    match target {
        Target::Real { ws, abs } => Ok((ws, abs)),
        _ => Err(format!("`{raw}` is not a workspace file")),
    }
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
            "no default workspace is set — `list @rumu:/workspaces` lists them; `invoke` \
             `workspace.use` on one to pick it",
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
pub(super) fn replace_occurrences(
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
        "delete" => p.tool_delete(state, &args, cx).await.finish(),
        "list" => p.tool_list(state, &args, cx).await.finish(),
        "bash" => p.tool_bash(state, &args, cx).await.finish(),
        "invoke" => p.tool_invoke(state, &args, cx).await.finish(),
        "search" => p.tool_search(state, &args, cx).await.finish(),
        "show" => p.tool_show(state, &args).await.finish(),
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
                let backend = self.backend_for(&ws);
                match backend.read_bytes(&abs).await {
                    Ok(bytes) if rumu_blob::is_image(&bytes) => {
                        // Effects are applied once, after the image path
                        // settles (its error branches are nested).
                        match self.read_image(&ws, &abs, bytes, offset, limit).await {
                            Ok(out) => Ok(out.with_effects(effects)),
                            Err(e) => Err(e.with_effects(effects)),
                        }
                    }
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes).into_owned();
                        match slice_lines(text, offset, limit) {
                            Ok(s) => Ok(ToolOutput::ok(s).with_effects(effects)),
                            Err(e) => Err(ToolError::model(e).with_effects(effects)),
                        }
                    }
                    Err(e) => {
                        Err(backend_err("read", Some(display(&ws, &abs)), e).with_effects(effects))
                    }
                }
            }
            Target::Rumu { abs } => {
                let (node, at) = self.route(&abs, state, cx)?;
                node.read(&at, offset, limit).await
            }
        }
    }

    /// An image read: ingest the bytes (normalized once, off the async
    /// executor) into the blob store and return the attachment, anchored
    /// and nothing else. `offset`/`limit` slice text lines — an image has
    /// none, so they are refused rather than silently ignored.
    async fn read_image(
        &self,
        ws: &Workspace,
        abs: &Utf8TypedPathBuf,
        bytes: Vec<u8>,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> ToolResult<ToolOutput> {
        if offset.is_some() || limit.is_some() {
            return Err(ToolError::model(
                "`offset`/`limit` do not apply to image reads",
            ));
        }
        let blobs = self
            .blobs()
            .map_err(|e| backend_err("read", Some(display(ws, abs)), e))?;
        let file_name = abs.file_name().map(str::to_owned);
        let at = display(ws, abs);
        let attachment = tokio::task::spawn_blocking(move || blobs.ingest_image(&bytes, file_name))
            .await
            .map_err(|e| ToolError::model(format!("reading image: {e}")))?
            .map_err(|e| backend_err("read", Some(at.clone()), e))?;
        // The call names the file already; text-only views render the
        // anchor as `[image: name W×H]`.
        Ok(ToolOutput::ok("[Image #1]").with_attachments([attachment]))
    }

    /// `show`: snapshot a file into the blob store and record it as an
    /// [`Artifact`] on the result.
    async fn tool_show(&self, state: &PathState, args: &Value) -> ToolResult<ToolOutput> {
        let args: ShowArgs = parse_args("show", args)?;
        let Target::Real { ws, abs } = target_of(state, &args.path)? else {
            return Err(ToolError::model(
                "`show` takes a workspace file; write the content to one (e.g. in `@scratch`) first",
            ));
        };
        let mut artifact = snapshot(&*self.backend_for(&ws), self.blobs(), &ws, &abs)
            .await
            .map_err(|e| ToolError::model(format!("show {}: {e}", display(&ws, &abs))))?;
        if let Some(title) = args
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        {
            artifact.title = title;
        }
        Ok(ToolOutput::ok(format!(
            "showed {} to the user ({} bytes)",
            artifact.path, artifact.size
        ))
        .with_data(&artifact))
    }

    /// Shared write preflight for real workspaces: the read-only check.
    fn write_target(&self, ws: &Workspace) -> ToolResult<()> {
        if ws.read_only {
            return Err(read_only_err(ws, "writes are refused"));
        }
        Ok(())
    }

    async fn tool_write(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: WriteArgs = parse_args("write", args)?;
        let target = target_of(state, &args.path)?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                if let Err(e) = self.write_target(&ws) {
                    return Err(e.with_effects(effects));
                }
                let backend = self.backend_for(&ws);
                // The UI diff payload: a create when the file is absent, an
                // update diffed against the previous content. Best effort —
                // a read failure or non-UTF-8 bytes leave it undiffed.
                let patch = match backend.read_bytes(&abs).await {
                    Ok(previous) => match String::from_utf8(previous) {
                        Ok(old) => patch::update(old, args.content.clone()).await,
                        Err(_) => patch::undiffed(),
                    },
                    Err(e) if is_not_found(&e) => patch::create(),
                    Err(_) => patch::undiffed(),
                };
                match backend.write_bytes(&abs, args.content.as_bytes()).await {
                    Ok(()) => Ok(ToolOutput::ok(format!(
                        "wrote {} ({} bytes)",
                        display(&ws, &abs),
                        args.content.len()
                    ))
                    .with_data(&patch.at(&ws, &abs))
                    .with_effects(effects)),
                    // Directories are never created for the model: a missing
                    // one usually means a mistyped path, not a new directory.
                    Err(e) if is_not_found(&e) => {
                        let fix = match colon_form(&args.path) {
                            Some(fixed) => format!("did you mean `{fixed}`?"),
                            None => "check the path (`<workspace>:<path>` for another \
                                     workspace), or create the directory with bash first"
                                .to_owned(),
                        };
                        Err(ToolError::model(format!(
                            "write {}: its directory does not exist; {fix}",
                            display(&ws, &abs)
                        ))
                        .with_effects(effects))
                    }
                    Err(e) => {
                        Err(backend_err("write", Some(display(&ws, &abs)), e).with_effects(effects))
                    }
                }
            }
            Target::Rumu { abs } => {
                let (node, at) = self.route(&abs, state, cx)?;
                node.write(&at, &args.content, args.from.as_deref()).await
            }
        }
    }

    async fn tool_edit(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: EditArgs = parse_args("edit", args)?;
        let target = target_of(state, &args.path)?;
        match target {
            Target::Real { ws, abs } => {
                let effects = self.maybe_inject_agents(cx, &ws).await;
                if let Err(e) = self.write_target(&ws) {
                    return Err(e.with_effects(effects));
                }
                let backend = self.backend_for(&ws);
                let text = match backend.read_bytes(&abs).await {
                    Ok(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => text,
                        Err(_) => {
                            return Err(ToolError::model(format!(
                                "edit {}: not a UTF-8 text file; use bash",
                                display(&ws, &abs)
                            ))
                            .with_effects(effects));
                        }
                    },
                    Err(e) => {
                        return Err(
                            backend_err("edit", Some(display(&ws, &abs)), e).with_effects(effects)
                        );
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
                        return Err(ToolError::model(e).with_effects(effects));
                    }
                };
                match backend.write_bytes(&abs, new_text.as_bytes()).await {
                    Ok(()) => {
                        let patch = patch::update(text, new_text).await;
                        Ok(ToolOutput::ok(format!(
                            "edited {} ({n} replacement{})",
                            display(&ws, &abs),
                            if n == 1 { "" } else { "s" }
                        ))
                        .with_data(&patch.at(&ws, &abs))
                        .with_effects(effects))
                    }
                    Err(e) => {
                        Err(backend_err("edit", Some(display(&ws, &abs)), e).with_effects(effects))
                    }
                }
            }
            Target::Rumu { abs } => {
                let (node, at) = self.route(&abs, state, cx)?;
                let all = args.replace_all.unwrap_or(false);
                node.edit(&at, &args.old_string, &args.new_string, all)
                    .await
            }
        }
    }

    /// `delete`: a file (its content kept in the patch, so a rewind can put
    /// it back), or whatever `@rumu` node serves deleting.
    async fn tool_delete(
        &self,
        state: &PathState,
        args: &Value,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let args: DeleteArgs = parse_args("delete", args)?;
        let (ws, abs) = match target_of(state, &args.path)? {
            Target::Real { ws, abs } => (ws, abs),
            Target::Rumu { abs } => {
                let (node, at) = self.route(&abs, state, cx)?;
                return node.delete(&at).await;
            }
        };
        let effects = self.maybe_inject_agents(cx, &ws).await;
        let shown = display(&ws, &abs);
        let result = async {
            if ws.read_only {
                return Err(read_only_err(&ws, "deletes are refused"));
            }
            let backend = self.backend_for(&ws);
            let old = match backend.read_bytes(&abs).await {
                Ok(bytes) => String::from_utf8(bytes).ok(),
                Err(e) if is_not_found(&e) => {
                    return Err(ToolError::model(format!("delete {shown}: no such file")));
                }
                Err(_) if backend.list_dir(&abs).await.is_ok() => {
                    return Err(ToolError::model(format!(
                        "delete {shown}: is a directory; `delete` removes files only, use bash \
                         for a directory"
                    )));
                }
                Err(e) => return Err(backend_err("delete", Some(shown.clone()), e)),
            };
            let patch = patch::delete(old).await;
            backend
                .remove_file(&abs)
                .await
                .map_err(|e| backend_err("delete", Some(shown.clone()), e))?;
            Ok(ToolOutput::ok(format!("deleted {shown}")).with_data(&patch.at(&ws, &abs)))
        }
        .await;
        match result {
            Ok(out) => Ok(out.with_effects(effects)),
            Err(e) => Err(e.with_effects(effects)),
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
                        Ok(ToolOutput::ok(body).with_effects(effects))
                    }
                    Err(e) => {
                        let hint = args
                            .path
                            .as_deref()
                            .and_then(colon_form)
                            .map(|fixed| format!("; did you mean `{fixed}`?"))
                            .unwrap_or_default();
                        let at = Some(display(&ws, &abs));
                        Err(backend_err("list", at, format!("{e}{hint}")).with_effects(effects))
                    }
                }
            }
            Target::Rumu { abs } => {
                let (node, at) = self.route(&abs, state, cx)?;
                node.listing(&at).await
            }
        }
    }

    async fn tool_bash(&self, state: &PathState, args: &Value, cx: &Cx) -> ToolResult<ToolOutput> {
        let args: BashArgs = parse_args("bash", args)?;
        let ws = bash_target(state, args.workspace.as_deref())?;
        if ws.read_only {
            return Err(read_only_err(&ws, "bash is disabled"));
        }
        let watch = match args.watch {
            Some(w) => Some(self.watch_for(cx, w, args.timeout, args.description.as_deref())?),
            None => None,
        };
        let effects = self.maybe_inject_agents(cx, &ws).await;
        let backend = self.backend_for(&ws);
        let command = with_scratch_dir(state, &ws, &args.command);
        // Stream every chunk to the UI (tool.progress) / the task buffer
        // (task.progress), and to the watch. The kernel applies `timeout` to a
        // foreground call (moving it to the background); a background one
        // without `watch` ends when the command exits, `task.kill` kills it,
        // or its owner aborts.
        let mut on_chunk = |chunk: &str| {
            cx.progress(Progress::Append(chunk.to_owned()));
            if let Some(watch) = &watch {
                watch.push(chunk);
            }
        };
        let exec: std::result::Result<ExecOutput, BackendError> = if let Some(watch) = &watch {
            let exec = backend.exec(&command, &mut on_chunk);
            match watch.run(exec, cx).await {
                Ok(exec) => exec,
                Err(ended) => return Ok(ToolOutput::ok(ended).with_effects(effects)),
            }
        } else {
            backend.exec(&command, &mut on_chunk).await
        };
        match exec {
            Ok(out) => Ok(BashOutcome {
                code: out.code,
                // A watch already posted the rest: the result (and so the
                // completion note) carries only what is still pending.
                output: last_lines(
                    match &watch {
                        Some(watch) => watch.tail(cx),
                        None => out.combined,
                    },
                    args.tail,
                ),
                effects,
                structured: cx.call().is_some_and(|c| c.parent.is_some()),
            }
            .into()),
            Err(e) => Err(backend_err("bash", None, e).with_effects(effects)),
        }
    }

    /// `invoke`: an MCP tool, or an action of an `@rumu` object checked
    /// against the actions its node declares.
    async fn tool_invoke(
        &self,
        state: &PathState,
        args: &Value,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let args: InvokeArgs = parse_args("invoke", args)?;
        let params = args.args.unwrap_or_default();
        if let Some((server, tool)) = args.action.split_once('/') {
            if args.path.is_some() {
                return Err(ToolError::model(format!(
                    "`{}` is an MCP tool: it takes no `path`",
                    args.action
                )));
            }
            let mcp = crate::mcp::servers(&self.decks, &self.mcp(), &cx.session, state);
            let blobs = || self.blobs();
            return crate::mcp::call(&self.mcp(), &mcp, state, blobs, server, tool, params).await;
        }
        // `invoke list on @rumu:/sessions`: the verbs are tools of their own.
        const VERBS: [&str; 8] = [
            "read", "write", "edit", "delete", "list", "search", "show", "bash",
        ];
        if VERBS.contains(&args.action.as_str()) {
            let action = &args.action;
            let with = match &args.path {
                Some(path) if ["read", "list", "delete", "show"].contains(&action.as_str()) => {
                    format!(" with `path: \"{path}\"`")
                }
                _ => String::new(),
            };
            return Err(ToolError::model(format!(
                "`{action}` is a tool, not an action: call `{action}` itself{with}"
            )));
        }
        let params = Value::Object(params);
        let Some(path) = args.path else {
            return Err(super::vfs::needs_path(&args.action));
        };
        let Target::Rumu { abs } = target_of(state, &path)? else {
            return Err(ToolError::model(format!(
                "`path` is the `{RUMU_WORKSPACE}` object an action acts on (`{RUMU_WORKSPACE}:/tasks/<id>`)"
            )));
        };
        let (node, at) = self.route(&abs, state, cx)?;
        let Some(spec) = node.actions().iter().find(|a| a.name == args.action) else {
            return Err(super::vfs::not_here(&args.action, &abs, node));
        };
        node.invoke(&at, spec, params).await
    }

    /// `workspace.use`: make `name` the branch's default workspace (the
    /// `workspace.use` marker the web's `branch.use_workspace` writes too)
    /// and list the workspaces as they then are.
    pub(super) async fn use_workspace(
        &self,
        state: &PathState,
        name: &str,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        if name != RUMU_WORKSPACE && !state.workspaces.iter().any(|w| w.name == name) {
            return Err(ToolError::model(unknown_workspace(state, name)));
        }
        let mut effects = Vec::new();
        if let Some(ws) = state.workspaces.iter().find(|w| w.name == name).cloned() {
            effects.extend(self.maybe_inject_agents(cx, &ws).await);
        }
        effects.push(Effect::marker(&WorkspaceUse {
            name: name.to_owned(),
        }));
        // The marker lands at the next safe point; report the listing as if
        // the new default were already in effect.
        let mut after = state.clone();
        after.default_workspace = Some(name.to_owned());
        Ok(ToolOutput::ok(format!(
            "default workspace set; workspaces:\n{}",
            self.listing(&after).await
        ))
        .with_effects(effects))
    }

    /// One line per workspace: the project's mounted ones first (marked),
    /// then the others, then the `@rumu` line. The default is marked `*`.
    pub(super) async fn listing(&self, state: &PathState) -> String {
        let mut ordered: Vec<&Workspace> = Vec::new();
        for name in &state.mounted {
            if let Some(ws) = state.workspaces.iter().find(|w| &w.name == name) {
                ordered.push(ws);
            }
        }
        for ws in &state.workspaces {
            if !state.mounted.contains(&ws.name) {
                ordered.push(ws);
            }
        }
        let mut lines = Vec::new();
        for ws in ordered {
            let star = if state.default_workspace.as_deref() == Some(ws.name.as_str()) {
                "* "
            } else {
                "  "
            };
            let rw = if ws.read_only { "ro" } else { "rw" };
            let mach = ws.label();
            let mut line = format!("{star}{} {mach} {} {rw}", ws.name, ws.root);
            if state.mounted.iter().any(|n| n == &ws.name) {
                line.push_str("  (mounted)");
            }
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
            "{star}{RUMU_WORKSPACE} rumu's own data (`list {RUMU_WORKSPACE}:/`); no bash"
        ));
        lines.join("\n")
    }
}

/// The variable a local `bash` finds the `@scratch` directory in.
const SCRATCH_DIR_ENV: &str = "RUMU_SCRATCH_DIR";

/// `command` with [`SCRATCH_DIR_ENV`] exported when it runs on this
/// machine, where the scratch directory is: in bash `@scratch` is only a
/// folder name. A command naming the variable creates the directory, as
/// using the workspace does.
fn with_scratch_dir(state: &PathState, ws: &Workspace, command: &str) -> String {
    let scratch = state
        .workspaces
        .iter()
        .find(|w| w.name == SCRATCH_WORKSPACE);
    let Some(scratch) = scratch.filter(|_| ws.ssh.is_none()) else {
        return command.to_owned();
    };
    if command.contains(SCRATCH_DIR_ENV) {
        let _ = std::fs::create_dir_all(scratch.root.as_str());
    }
    format!(
        "export {SCRATCH_DIR_ENV}={}\n{command}",
        sh_quote(scratch.root.as_str())
    )
}

/// The canonical display form of a real-workspace path.
/// `@scratch/<rest>` or a bare `@scratch` written for `@scratch:<rest>`
/// (likewise `@rumu`): the colon form, to point the model at its mistake.
/// The path is not rewritten.
fn colon_form(raw: &str) -> Option<String> {
    [SCRATCH_WORKSPACE, RUMU_WORKSPACE].iter().find_map(|name| {
        let rest = raw.strip_prefix(name)?;
        let rest = rest.strip_prefix('/').or(rest.is_empty().then_some(rest))?;
        Some(format!("{name}:{rest}"))
    })
}

fn display(ws: &Workspace, abs: &Utf8TypedPathBuf) -> String {
    rumu_core::display(&ws.name, ws.root.to_path(), abs.to_path())
}

// ---------------------------------------------------------------------------
// the `@rumu` virtual filesystem (routed by `vfs`)
// ---------------------------------------------------------------------------

impl WorkspacePlugin {
    /// The `search` tool: cards by default, past conversations with
    /// `scope: "@rumu:/sessions[/<session>]"`. Read-only — no `card.use`
    /// markers.
    async fn tool_search(
        &self,
        state: &PathState,
        args: &Value,
        cx: &Cx,
    ) -> ToolResult<ToolOutput> {
        let args: SearchArgs = parse_args("search", args)?;
        let limit = args.limit.map(|l| l.max(1) as usize);
        let scope = args.scope.as_deref().unwrap_or("");
        // An omitted scope means the whole card library; a given one is a
        // qualified path prefix that must land in the `@rumu` workspace.
        let abs: Utf8TypedPathBuf = if scope.is_empty() {
            Utf8TypedPathBuf::from_unix("/cards")
        } else {
            match rumu_core::resolve_state(state, scope) {
                Ok(addr) if addr.ws == RUMU_WORKSPACE => addr.abs,
                _ => return Err(bad_scope()),
            }
        };
        let Utf8TypedPathBuf::Unix(abs) = abs else {
            return Err(bad_scope());
        };
        let (node, at) = self.route(&abs, state, cx).map_err(|_| bad_scope())?;
        node.search(&at, &args.query, limit).await
    }
}
