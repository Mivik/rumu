//! The model-facing tools: schemas, `<ws>:<path>` target resolution and the
//! implementations over the workspace backends. The `@rumu`
//! virtual workspace (`/cards`, `/history`, `/sessions`) is routed from here
//! into [`cards`] (and, for `/sessions`, `session`).

use std::time::Duration;

use rumu_cards::usage::CardOp;
use rumu_cards::{Cards, card};
use rumu_core::{
    Artifact, ArtifactKind, Effect, INVOKE_OUTPUT_LIMIT, Manifest, PathState, Progress,
    RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, ToolError, ToolOutput, ToolResult,
    ToolResultExt, ToolSpec, Utf8TypedPathBuf, Utf8UnixPath, Utf8UnixPathBuf, Workspace,
    WorkspaceUse, parse_args, resolve_state, tool_schema, truncate_middle,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::backend::{BackendError, ExecOutput, is_not_found, probe_uname};
use super::patch;
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

const BASH_TIMEOUT_DEFAULT: u64 = 60;
const BASH_TIMEOUT_MAX: u64 = 600;
pub(super) const LIST_LIMIT: usize = 100;
/// The largest file `show` snapshots.
const ARTIFACT_MAX_BYTES: usize = 5 * 1024 * 1024;

const INSTRUCTIONS: &str = "Files live in workspaces. A workspace is a name for a root \
directory on this machine or an ssh host; the name is not a folder and never appears inside a \
path. A tool `path` is either a plain path in the default workspace (relative to its root, or \
absolute on its machine), or `<workspace>:<path>` for any workspace — the colon separates the \
name from the path:\n\
- `src/main.rs`, `/etc/hosts` → the default workspace\n\
- `app:/var/log/app.log` → workspace `app`\n\
- `@scratch:page.html`, `@scratch:` (its root) → the scratch workspace\n\
- `@scratch/page.html` → a folder named `@scratch` in the default workspace, NOT the scratch \
workspace\n\
Name files the same way when you mention them to the user. \
`bash` runs in the default workspace's root (or `workspace`'s), so relative paths match the file \
tools. `workspace_use(name)` changes the default; `workspace_use()` lists workspaces. Read-only \
workspaces refuse writes and bash.\n\n\
`@scratch` is this session's throwaway workspace on this machine, shared with its branches and \
sub-agents and deleted with the session. Use it for scripts, downloads, clones and generated \
files; what should outlive the session belongs in a card or a project workspace.\n\n\
To give the user a page, document, diagram or code to look at, write it to a file (usually in \
`@scratch`) and `show` it; `show` it again after a change for a new version. HTML runs sandboxed \
with no network: inline its scripts, styles and images.\n\n\
`@rumu` holds rumu's own data (no bash):\n\
- `@rumu:/cards/<mount>/<path>`: cards of the decks mounted on this branch (`/cards/self` is \
yours). `read` gives an editable view (YAML frontmatter, then the body); a write is refused if \
the card changed since your read — re-read and merge. Card edits survive a rewind.\n\
- `@rumu:/workspaces[/<name>]`: the workspace registry; `@rumu:/project/workspaces`: the \
project's ordered list (read it before writing).\n\
- `@rumu:/history`: the decks' change timeline.\n\
- `@rumu:/sessions/<session>/<node>` and `.../<from>..<to>`: past conversations, read-only \
(yours and the mounted decks'; `@rumu:/sessions/by-assistant/<deck id>` lists one assistant's). \
`search(query, scope: \"@rumu:/sessions\")` searches them.\n\
`card_mount` mounts and unmounts decks; mounting a project deck at `/cards/project` moves the \
session into that project (`card_mount(create: <name>, at: \"project\")` creates one).";

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
struct BashArgs {
    /// shell snippet
    command: String,
    /// the call's intent, 5-10 words in active voice ("List files in src",
    /// "Run the parser tests"); shown as the call's summary in the transcript
    #[allow(dead_code)] // read from the raw args by the UI and the task label
    description: Option<String>,
    /// workspace name; default: the branch default
    workspace: Option<String>,
    /// seconds, default 60, max 600
    timeout: Option<u64>,
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
    /// `@rumu:/cards/<prefix>`, `@rumu:/sessions[/<session>]` or `@rumu:/tools[/<server>]`
    scope: Option<String>,
    /// max hits (default 10; 5 for tools)
    limit: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WorkspaceUseArgs {
    /// workspace name (any registered workspace, `@scratch`, or `@rumu`)
    name: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CardMountArgs {
    /// deck id (from the listing); omit to list decks
    pub(super) deck: Option<String>,
    /// mount point under `/cards` (`/cards/user`); with no `deck`, unmounts it
    pub(super) at: Option<String>,
    /// create a project deck with this name and mount it at `/cards/project`
    /// (only with `at: "project"`, never with `deck`)
    pub(super) create: Option<String>,
}

/// Parse `card_mount` arguments.
pub(super) fn parse_mount_args(args: &Value) -> ToolResult<CardMountArgs> {
    parse_args("card_mount", args)
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
image (PNG, JPEG, GIF, WebP) as an image attachment beside a one-line header naming the file, \
its dimensions and stored size. `path`: a plain path (default workspace) or \
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
rejected on read-only workspaces). Same `path` addressing as `read`; for cards the \
content is the editable view — frontmatter + body. With `from`, moves that card to `path` \
(the destination must not exist). Writing to a deck other than your own asks the user unless \
they already allowed it."
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
                name: "list".into(),
                description: "List a directory's entries, one per line (directories \
get a trailing `/`). Omitted `path` = the default workspace's root; `proj:` / \
`@rumu:` (nothing after the colon) = that workspace's root. On the `@rumu` \
workspace, lists cards."
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
stdout+stderr (truncated to ~20k chars) and the exit code. Default timeout 60s (max \
600s). Disabled on read-only workspaces and on the `@rumu` workspace. Give every call a \
`description` (5-10 words, active voice): the transcript shows it as the call's summary."
                    .into(),
                schema: tool_schema::<BashArgs>(),
                background: true,
                output_schema: Some(tool_schema::<BashResult>()),
                parallel: false,
            },
            ToolSpec {
                name: "show".into(),
                description: "Show a text file to the user as an artifact: a snapshot rendered \
beside the chat by its extension (`.html` runs sandboxed with no network; `.md`, `.svg`; \
anything else as highlighted code). Same `path` addressing as `read`; not on `@rumu`. Showing \
the same path again adds a version."
                    .into(),
                schema: tool_schema::<ShowArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "search".into(),
                description: "Search cards (default), past conversations or MCP tools by text. \
All are relevance-ranked and match any term. `scope` narrows it: `@rumu:/cards/<mount>/<prefix>` \
for a mounted deck's cards, `@rumu:/sessions` for all past conversations you can see (yours and \
mounted assistants'), `@rumu:/sessions/by-assistant/<deck id>` for one assistant's, \
`@rumu:/sessions/<session>` for one. Cards and conversations return addresses with short \
snippets, never full text — follow up with `read`. `@rumu:/tools[/<server>]` searches the \
session's MCP tools; each hit shows its input schema, ready for `call_tool`."
                    .into(),
                schema: tool_schema::<SearchArgs>(),
                background: false,
                output_schema: None,
                parallel: true,
            },
            ToolSpec {
                name: "card_mount".into(),
                description: "Manage the branch's card mounts. Without arguments, list decks \
(name, id, mount point, whether it is an assistant or a project, and whether mounting or \
writing it asks the user). With `deck` and `at`, mount it at `/cards/<at>` (the user approves \
unless they already allowed it); only project decks mount at `/cards/project`, and mounting one \
there moves the session into that project. With `at` alone, unmount (`at: \"project\"` leaves \
the project). With `create` and `at: \"project\"`, create a project deck and move into it. Mount \
points must not overlap. The registry is `@rumu:/workspaces`; the session project's list is \
`@rumu:/project/workspaces`. Only mounted decks feed the catalog, recall, list, search and \
notices."
                    .into(),
                schema: tool_schema::<CardMountArgs>(),
                background: false,
                output_schema: None,
                parallel: false,
            },
            ToolSpec {
                name: "workspace_use".into(),
                description: "Set the default workspace for subsequent tool calls \
(branch state; forks inherit it) and return the workspace listing (the project's mounted \
workspaces first, then the others). Without `name`, just list workspaces. `name` may be any \
registered workspace, `@scratch`, or `@rumu`."
                    .into(),
                schema: tool_schema::<WorkspaceUseArgs>(),
                background: false,
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
            "nothing is mounted; use card_mount".to_owned()
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

/// `unknown path ...`.
pub(super) fn unknown_rumu_path(abs: &Utf8UnixPath) -> ToolError {
    ToolError::model(format!(
        "unknown path `{RUMU_WORKSPACE}:{abs}` (roots: /cards, /history, /sessions, /tasks, /tools)"
    ))
}

/// The `search` scope refusal.
fn bad_scope() -> ToolError {
    ToolError::model(format!(
        "`scope` must address the `{RUMU_WORKSPACE}` workspace: \
         `{RUMU_WORKSPACE}:/cards/<prefix>`, `{RUMU_WORKSPACE}:/sessions[/<session>]` or \
         `{RUMU_WORKSPACE}:/tools[/<server>]`"
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
        "list" => p.tool_list(state, &args, cx).await.finish(),
        "bash" => p.tool_bash(state, &args, cx).await.finish(),
        "workspace_use" => p.tool_use(state, &args, cx).await.finish(),
        "card_mount" => p.tool_card_mount(state, &args, cx).await.finish(),
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
            Target::Rumu { abs } => self.rumu_read(&abs, offset, limit, cx).await,
        }
    }

    /// An image read: ingest the bytes (normalized once, off the async
    /// executor) into the blob store and return the attachment beside a
    /// one-line header. `offset`/`limit` slice text lines — an image has
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
        Ok(ToolOutput::ok(format!(
            "image {at} ({}×{}, {} bytes stored)",
            attachment.width, attachment.height, attachment.size
        ))
        .with_attachments([attachment]))
    }

    /// `show`: snapshot a text file into the blob store and record it as an
    /// [`Artifact`] on the result.
    async fn tool_show(&self, state: &PathState, args: &Value) -> ToolResult<ToolOutput> {
        let args: ShowArgs = parse_args("show", args)?;
        let Target::Real { ws, abs } = target_of(state, &args.path)? else {
            return Err(ToolError::model(
                "`show` takes a workspace file; write the content to one (e.g. in `@scratch`) first",
            ));
        };
        let at = display(&ws, &abs);
        let bytes = self
            .backend_for(&ws)
            .read_bytes(&abs)
            .await
            .map_err(|e| backend_err("show", Some(at.clone()), e))?;
        if bytes.len() > ARTIFACT_MAX_BYTES {
            return Err(ToolError::model(format!(
                "show {at}: {} bytes is over the {ARTIFACT_MAX_BYTES}-byte limit",
                bytes.len()
            )));
        }
        if std::str::from_utf8(&bytes).is_err() {
            return Err(ToolError::model(format!(
                "show {at}: not a UTF-8 text file (images: `read` them)"
            )));
        }
        let hash = self
            .blobs()
            .and_then(|blobs| blobs.put(&bytes))
            .map_err(|e| backend_err("show", Some(at.clone()), e))?;
        let name = abs.file_name().unwrap_or(at.as_str()).to_owned();
        let title = args
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| name.clone());
        let artifact = Artifact {
            kind: ArtifactKind::of(&name),
            path: at,
            title,
            hash,
            size: bytes.len() as u64,
        };
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
                    .with_data(&patch)
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
                if super::registry::is_config_path(&abs) {
                    return self
                        .config_write(state, &abs, &args.content, args.from.as_deref(), cx)
                        .await;
                }
                let Some(path) = card_ref_of(&abs) else {
                    return Err(self.rumu_write_refused(&abs));
                };
                self.card_write(path, &args.content, CardOp::Write, args.from.as_deref(), cx)
                    .await
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
                        .with_data(&patch)
                        .with_effects(effects))
                    }
                    Err(e) => {
                        Err(backend_err("edit", Some(display(&ws, &abs)), e).with_effects(effects))
                    }
                }
            }
            Target::Rumu { abs } => {
                if super::registry::is_config_path(&abs) {
                    return self
                        .config_edit(
                            state,
                            &abs,
                            &args.old_string,
                            &args.new_string,
                            args.replace_all.unwrap_or(false),
                            cx,
                        )
                        .await;
                }
                // Edit = string replace on the view, then the write path. The
                // read-freshness check comes first: a stale read is the more
                // fundamental problem than a non-matching string.
                let Some(path) = card_ref_of(&abs) else {
                    return Err(self.rumu_write_refused(&abs));
                };
                if cx.state()?.asked_by.is_some() {
                    return Err(ToolError::model(
                        "you are answering a question; cards cannot change here",
                    ));
                }
                let cards = self.cards(cx)?;
                if let Some(err) = invalid_card_ref(&cards, path) {
                    return Err(err);
                }
                let existing = match cards.raw(path) {
                    Ok(Some(c)) => c,
                    Ok(None) => return Err(no_card_err(path, &[])),
                    Err(e) => return Err(card_load_err(path, e)),
                };
                if existing.link().is_some() {
                    return Err(ToolError::model(format!(
                        "card `{}` is a link; write its target instead",
                        rumu_cards::card_uri(path)
                    )));
                }
                if let Some(err) = self.ensure_fresh_read(&cards, path, &existing, cx) {
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
                    Err(e) => return Err(ToolError::model(e)),
                };
                let mut out = self
                    .card_write(path, &new_view, CardOp::Edit, None, cx)
                    .await?;
                if out.is_error {
                    return Ok(out);
                }
                // Keep the write's diff payload; only the text is edit-specific.
                out.content.text = format!(
                    "edited card `{}` ({n} replacement{})",
                    rumu_cards::card_uri(path),
                    if n == 1 { "" } else { "s" }
                );
                Ok(out)
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
            return Err(read_only_err(&ws, "bash is disabled"));
        }
        let effects = self.maybe_inject_agents(cx, &ws).await;
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
                            "`{}` timed out after {timeout}s",
                            args.command
                        ))
                        .with_effects(effects));
                    }
                }
            };
        match exec {
            Ok(out) => Ok(BashOutcome {
                code: out.code,
                output: out.combined,
                effects,
                structured: cx.call().is_some_and(|c| c.parent.is_some()),
            }),
            Err(e) => Err(backend_err("bash", None, e).with_effects(effects)),
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
            "default workspace set; workspaces:\n{}",
            self.listing(&after).await
        ))
        .with_effects(effects))
    }

    /// One line per workspace: the project's mounted ones first (marked),
    /// then the others, then the `@rumu` line. The default is marked `*`.
    async fn listing(&self, state: &PathState) -> String {
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
            "{star}{RUMU_WORKSPACE} rumu's own data: {RUMU_WORKSPACE}:/cards (knowledge cards), \
             {RUMU_WORKSPACE}:/workspaces (the registry), {RUMU_WORKSPACE}:/sessions (past \
             conversations); no bash"
        ));
        lines.join("\n")
    }
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

/// The card path a `/cards/<mount>/<path>[.md]` address names, if it is one.
pub(super) fn card_ref_of(abs: &Utf8UnixPath) -> Option<&str> {
    let path = abs.strip_prefix("/cards").ok()?.as_str();
    let path = path.trim_start_matches('/');
    let path = path.strip_suffix(".md").unwrap_or(path);
    (!path.is_empty()).then_some(path)
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
        if super::registry::is_config_path(abs) {
            return self.config_read(abs, offset, limit, cx).await;
        }
        if abs.starts_with("/history") {
            return self.history_read(abs, offset, limit, cx).await;
        }
        if let Some(path) = card_ref_of(abs) {
            return self.card_read(path, offset, limit, cx).await;
        }
        if abs.starts_with("/sessions") {
            return self.session_read(abs, offset, limit, cx).await;
        }
        if abs.starts_with("/tasks") {
            return self.task_read(abs, offset, limit, cx).await;
        }
        if crate::mcp::is_tools_path(abs) {
            let out = crate::mcp::read(&self.mcp(), &*cx.state()?, abs).await?;
            return slice_lines(out.content.text, offset, limit)
                .map(ToolOutput::ok)
                .map_err(ToolError::model);
        }
        Err(unknown_rumu_path(abs))
    }

    /// The refusal for writes outside `/cards/<mount>/<path>`: `/sessions` is
    /// read-only, everything else is unknown.
    fn rumu_write_refused(&self, abs: &Utf8UnixPath) -> ToolError {
        if abs.starts_with("/sessions") {
            ToolError::model("`@rumu:/sessions` is read-only")
        } else if abs.starts_with("/history") {
            ToolError::model("`@rumu:/history` is read-only")
        } else {
            unknown_rumu_path(abs)
        }
    }

    /// `list` inside the `@rumu` workspace.
    async fn rumu_list(&self, abs: &Utf8UnixPath, cx: &Cx) -> ToolResult<ToolOutput> {
        if super::registry::is_config_path(abs) {
            return self.config_list(abs, cx).await;
        }
        match abs.as_str() {
            "/" => {
                let mut roots =
                    "cards/\nhistory/\nsessions/\ntasks/\ntools/\nworkspaces/".to_owned();
                if cx.state()?.project.is_some() {
                    roots.push_str("\nproject/");
                }
                Ok(roots.into())
            }
            "/cards" => self.card_list("", cx).await,
            _ if abs.starts_with("/cards") => {
                let prefix = abs
                    .strip_prefix("/cards")
                    .map(Utf8UnixPath::as_str)
                    .unwrap_or("");
                self.card_list(prefix, cx).await
            }
            _ if abs.starts_with("/history") => self.history_list(abs, cx).await,
            _ if abs.starts_with("/sessions") => self.session_list(abs, cx).await,
            _ if abs.starts_with("/tasks") => self.task_read(abs, None, None, cx).await,
            _ if crate::mcp::is_tools_path(abs) => {
                crate::mcp::list(&self.mcp(), &*cx.state()?, abs).await
            }
            _ => Err(unknown_rumu_path(abs)),
        }
    }

    /// The `search` tool: cards by default, past conversations with
    /// `scope: "@rumu:/sessions[/<session>]"`. Read-only — no `card_use`
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
        if crate::mcp::is_tools_path(&abs) {
            let limit = limit.unwrap_or(crate::mcp::SEARCH_LIMIT);
            return crate::mcp::search(&self.mcp(), state, &abs, &args.query, limit).await;
        }
        let limit = limit.unwrap_or(10);
        if abs.starts_with("/sessions") {
            return self.session_search(&args.query, &abs, limit, cx).await;
        }
        if abs.starts_with("/cards") {
            let prefix = abs
                .strip_prefix("/cards")
                .map(Utf8UnixPath::as_str)
                .unwrap_or("");
            return rumu_cards::search_text(
                &self.cards(cx)?,
                &args.query,
                &json!({ "prefix": prefix, "limit": limit }),
            );
        }
        Err(bad_scope())
    }
}
