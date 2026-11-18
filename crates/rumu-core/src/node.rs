use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BranchId, NodeId, SCRATCH_INBOX, SCRATCH_WORKSPACE, SessionId, Timestamp, Workspace};

/// A persisted event. All nodes of a session form a tree via `parent`.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Node {
    pub id: NodeId,
    pub parent: Option<NodeId>,
    pub session: SessionId,
    /// End time of the producing operation (append-on-complete).
    pub at: Timestamp,
    /// Wall time of the operation that produced this node, ms. `at` is the end
    /// time, so the start is `at - dur_ms`; instantaneous nodes (user, note,
    /// marker) leave it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur_ms: Option<u64>,
    pub body: NodeBody,
}

/// Tag of the `write`/`edit` result payload the UI renders as a file diff.
pub const FILE_PATCH_TAG: &str = "workspace.file_patch";

/// Versioned structured payload. Readers that don't know `tag` or don't support
/// `v` fall back to the node's text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Tagged {
    pub tag: String,
    pub v: u32,
    pub data: Value,
}

/// A payload type with a stable tag and version, stored in [`Tagged`].
pub trait Structured: Serialize + DeserializeOwned {
    const TAG: &'static str;
    const V: u32;
}

impl Tagged {
    /// Encode a structured payload.
    pub fn new<T: Structured>(data: &T) -> Self {
        Self {
            tag: T::TAG.to_owned(),
            v: T::V,
            data: serde_json::to_value(data).unwrap_or(Value::Null),
        }
    }

    /// Decode the payload, or `None` on a tag/version mismatch or a parse error.
    pub fn decode<T: Structured>(&self) -> Option<T> {
        if self.tag != T::TAG || self.v != T::V {
            return None;
        }
        serde_json::from_value(self.data.clone()).ok()
    }
}

/// The note `source` for notes written by rumu itself (the kernel), not by a
/// plugin.
pub const SYSTEM_SOURCE: &str = "system";

/// Everything projected to the model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Content {
    /// Frozen at write time; with `attachments`, the ONLY thing the model
    /// sees. An `[Image #N]` anchor marks where the N-th attachment sits
    /// (see [`Content::segments`]).
    pub text: String,
    /// Images, in anchor-number order. Each names a blob in the
    /// content-addressed blob store, normalized once at ingest so its bytes
    /// (and so the projection) never change afterwards.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    /// For the UI and plugins; never projected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Tagged>,
}

/// What kind of media an [`Attachment`] holds. These are the primitives a
/// model can consume directly; video is not one (it is frames, optionally
/// with an audio track), and documents are compiled by the harness into page
/// images plus text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AttachmentKind {
    #[default]
    Image,
    Audio,
}

/// A reference to normalized media in the blob store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Attachment {
    /// The media kind; absent in records written before it existed (image).
    #[serde(default)]
    pub kind: AttachmentKind,
    /// Lowercase hex SHA-256 of the stored bytes; the blob's file name.
    pub hash: String,
    /// MIME type of the stored bytes (`image/png`, `image/jpeg`).
    pub mime: String,
    /// Original file name, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Stored size in bytes.
    pub size: u64,
}

impl Attachment {
    /// The stand-in text for readers that cannot show the media (transcripts,
    /// search, a blob gone missing): `[image: name 800×600]`.
    pub fn placeholder(&self) -> String {
        let at = match &self.name {
            Some(name) => format!("{name} {}×{}", self.width, self.height),
            None => format!("{}×{}", self.width, self.height),
        };
        match self.kind {
            AttachmentKind::Image => format!("[image: {at}]"),
            AttachmentKind::Audio => format!("[audio: {at}]"),
        }
    }

    /// Where a prompt's attachment is copied inside the session's `@scratch`
    /// so tools can work on it: `.inbox/attachments/<hash prefix>.<ext>`.
    pub fn scratch_path(&self) -> String {
        let ext = match self.mime.split_once('/') {
            Some((_, "jpeg")) => "jpg",
            Some((_, sub)) => sub,
            None => "bin",
        };
        let stem = self.hash.get(..12).unwrap_or(&self.hash);
        format!("{SCRATCH_INBOX}/attachments/{stem}.{ext}")
    }
}

/// The in-text anchor of the `n`-th (1-based) attachment: `[Image #n]`.
pub fn image_anchor(n: usize) -> String {
    format!("[Image #{n}]")
}

/// The label projected ahead of the `n`-th image, so the model can refer to
/// it by name: `Image n:`, or `Image n (@scratch:<path>):` when the image
/// is also a file there.
pub fn image_label(n: usize, file: Option<&str>) -> String {
    match file {
        Some(path) => format!("Image {n} ({SCRATCH_WORKSPACE}:{path}):"),
        None => format!("Image {n}:"),
    }
}

/// One piece of a [`Content`], in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    /// The `n`-th (1-based) attachment. `anchored` is false for one whose
    /// anchor is missing from the text; those follow all of the text.
    Image {
        n: usize,
        attachment: &'a Attachment,
        anchored: bool,
    },
}

impl Content {
    /// Plain text content.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            ..Self::default()
        }
    }

    /// Add images to the content.
    pub fn with_attachments(mut self, attachments: impl IntoIterator<Item = Attachment>) -> Self {
        self.attachments.extend(attachments);
        self
    }

    /// Split the content into text and images, in reading order: the first
    /// `[Image #N]` anchor of each attachment (1 ≤ N ≤ count, no leading
    /// zero) is replaced by that image; repeated or out-of-range anchors stay
    /// text. Attachments without an anchor follow the text, in order. This is
    /// the one placement rule — the model projection, transcripts and the UI
    /// all follow it.
    pub fn segments(&self) -> Vec<Segment<'_>> {
        const OPEN: &str = "[Image #";
        let text = self.text.as_str();
        let mut placed = vec![false; self.attachments.len()];
        let mut out = Vec::new();
        let (mut last, mut scan) = (0, 0);
        while let Some(off) = text[scan..].find(OPEN) {
            let at = scan + off;
            let digits = at + OPEN.len();
            let len = text[digits..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let end = digits + len;
            let n = (len > 0 && !text[digits..].starts_with('0') && text[end..].starts_with(']'))
                .then(|| text[digits..end].parse::<usize>().ok())
                .flatten()
                .filter(|n| (1..=placed.len()).contains(n) && !placed[n - 1]);
            let Some(n) = n else {
                scan = at + 1;
                continue;
            };
            if last < at {
                out.push(Segment::Text(&text[last..at]));
            }
            placed[n - 1] = true;
            out.push(Segment::Image {
                n,
                attachment: &self.attachments[n - 1],
                anchored: true,
            });
            last = end + 1;
            scan = last;
        }
        if last < text.len() {
            out.push(Segment::Text(&text[last..]));
        }
        for (i, attachment) in self.attachments.iter().enumerate() {
            if !placed[i] {
                out.push(Segment::Image {
                    n: i + 1,
                    attachment,
                    anchored: false,
                });
            }
        }
        out
    }

    /// The text with every image as its [`Attachment::placeholder`], for
    /// text-only readers (transcripts, search).
    pub fn rendered(&self) -> String {
        if self.attachments.is_empty() {
            return self.text.clone();
        }
        let mut out = String::new();
        for segment in self.segments() {
            match segment {
                Segment::Text(t) => out.push_str(t),
                Segment::Image {
                    attachment,
                    anchored,
                    ..
                } => {
                    if !anchored && !out.is_empty() && !out.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str(&attachment.placeholder());
                }
            }
        }
        out
    }

    /// Note text plus a structured payload.
    pub fn note_with<T: Structured>(s: impl Into<String>, data: &T) -> Self {
        Self::text(s).with_data(data)
    }

    /// A marker: no text (never projected) plus a structured payload, visible
    /// only through the node index and structured readers.
    pub fn marker<T: Structured>(data: &T) -> Self {
        Self {
            data: Some(Tagged::new(data)),
            ..Self::default()
        }
    }

    /// Plain text plus a structured payload.
    pub fn with_data<T: Structured>(mut self, data: &T) -> Self {
        self.data = Some(Tagged::new(data));
        self
    }

    /// Decode the structured payload, if it has the expected tag and version.
    pub fn decode<T: Structured>(&self) -> Option<T> {
        self.data.as_ref()?.decode()
    }

    /// True when this is a marker: nothing projected, only a payload.
    pub fn is_marker(&self) -> bool {
        self.text.is_empty() && self.attachments.is_empty() && self.data.is_some()
    }

    /// Validate that a note carries either projected text or a structured
    /// payload. Empty-text markers are meaningful only with `data`; a note
    /// with neither is rejected at the [`crate::Inbox`] boundary.
    pub fn validate_note(&self) -> crate::Result<()> {
        if self.text.is_empty() && self.data.is_none() {
            return Err(crate::Error::EmptyNote);
        }
        Ok(())
    }
}

impl From<&str> for Content {
    fn from(text: &str) -> Self {
        Self::text(text)
    }
}

impl From<String> for Content {
    fn from(text: String) -> Self {
        Self::text(text)
    }
}

/// One thinking block of a model reply, in the order the reply gave them.
/// Anthropic accepts a replayed block only with the exact text and opaque
/// signature it produced, ahead of the text and tool calls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum ReasoningBlock {
    /// Visible (or summarized) thinking text. `signature` is the opaque
    /// continuation token; without one the block is display-only.
    Thinking {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A provider-native opaque block, replayed verbatim (Anthropic
    /// `redacted_thinking`).
    Redacted { data: Value },
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum NodeBody {
    User {
        content: Content,
    },
    Assistant {
        content: Content,
        /// Thinking blocks, in the reply's original order. Empty on replies
        /// without reasoning.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        reasoning: Vec<ReasoningBlock>,
        #[serde(flatten)]
        #[ts(flatten)]
        stats: CallStats,
        /// The provider could not use earlier reasoning on this request (it
        /// no longer verified against the conversation, or another model
        /// produced it), so the reasoning of every assistant node before this
        /// one is never replayed again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reasoning_reset: bool,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
        model: String,
        #[serde(default)]
        usage: Usage,
    },
    ToolResult {
        call_id: String,
        content: Content,
        #[serde(default)]
        is_error: bool,
        /// Set when a plugin (e.g. a `code` script) made the call through
        /// `Cx::invoke`: never projected to the model; the UI nests it under
        /// `parent`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        invoke: Option<Invoke>,
    },
    /// Plugin-written context (memory cards, workspace info, reminders,
    /// the card catalog). Non-empty `content.text` projects as
    /// a user-side `<note source="..">` fragment; empty text means a marker —
    /// not projected to the model, visible only through `content.data`.
    Note {
        source: String,
        content: Content,
    },
}

/// A tool call a plugin made through `Cx::invoke`, nested under the model
/// call that triggered the plugin. Carried on its [`NodeBody::ToolResult`].
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Invoke {
    /// The call id of the enclosing model tool call.
    pub parent: String,
    /// The effective tool name (after any `tool.pre` rewrite).
    pub name: String,
    /// The effective args (after any `tool.pre` rewrite).
    pub args: Value,
    /// The hook's block reason when `tool.pre` blocked the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<String>,
}

/// How an artifact renders, picked from its file extension (and, for the
/// text kinds, its bytes being UTF-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ArtifactKind {
    /// A page, run in a sandboxed frame with no network.
    Html,
    Markdown,
    Svg,
    /// Any other text, highlighted by its extension.
    Code,
    Image,
    Audio,
    Video,
    Pdf,
    /// Anything else: offered for download only.
    File,
}

/// Extensions a browser displays or plays as themselves, with the media
/// type to serve them as. None of these types runs script on the page.
const MEDIA_TYPES: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("flac", "audio/flac"),
    ("ogg", "audio/ogg"),
    ("oga", "audio/ogg"),
    ("opus", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
    ("mp4", "video/mp4"),
    ("m4v", "video/mp4"),
    ("mov", "video/quicktime"),
    ("webm", "video/webm"),
    ("ogv", "video/ogg"),
    ("pdf", "application/pdf"),
];

/// The media type a file named `name` displays or plays as, if it is one.
pub fn media_type(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    MEDIA_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, mime)| *mime)
}

/// Whether `mime` is one [`media_type`] gives: safe to serve as itself.
pub fn is_media_type(mime: &str) -> bool {
    MEDIA_TYPES.iter().any(|(_, m)| *m == mime)
}

impl ArtifactKind {
    /// The kind of a file named `name`; `text` when its bytes are UTF-8.
    pub fn of(name: &str, text: bool) -> Self {
        if let Some(mime) = media_type(name) {
            return match mime.split_once('/').map_or("", |(top, _)| top) {
                "image" => Self::Image,
                "audio" => Self::Audio,
                "video" => Self::Video,
                _ => Self::Pdf,
            };
        }
        if !text {
            return Self::File;
        }
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("html" | "htm") => Self::Html,
            Some("md" | "markdown") => Self::Markdown,
            Some("svg") => Self::Svg,
            _ => Self::Code,
        }
    }
}

/// Payload of a `show` result (v1): a snapshot of a workspace file shown to
/// the user. The bytes are a blob, so the branch keeps this version whatever
/// later happens to the file; the snapshots of one `path` on a path are its
/// versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Artifact {
    /// The file it was taken from, workspace-qualified.
    pub path: String,
    pub title: String,
    pub kind: ArtifactKind,
    /// Lowercase hex SHA-256 of the bytes in the blob store.
    pub hash: String,
    pub size: u64,
    /// The media type to serve the blob as ([`media_type`]), for the image,
    /// audio, video and PDF kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mime: Option<String>,
}

impl Structured for Artifact {
    const TAG: &'static str = "artifact";
    const V: u32 = 1;
}

/// Payload of the `workspace.use` marker (v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WorkspaceUse {
    pub name: String,
}

impl Structured for WorkspaceUse {
    const TAG: &'static str = "workspace.use";
    const V: u32 = 1;
}

/// A file on a workspace: its name and the file's absolute path on its
/// machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FileRef {
    pub ws: String,
    pub path: String,
}

/// Payload of the `workspace.revert` marker (v1): the user reverted the
/// file changes made since `since` (the time of the user message they
/// rewound to); `files` are the ones put back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct FilesReverted {
    pub since: Timestamp,
    pub files: Vec<FileRef>,
}

impl Structured for FilesReverted {
    const TAG: &'static str = "workspace.revert";
    const V: u32 = 1;
}

/// Payload of the `workspace.set` marker (v1): a sub-agent's
/// `inherit.workspaces`, narrowing the workspaces the branch may address from
/// here on (every registered one otherwise). The built-in `@rumu` and
/// `@scratch` workspaces are always addressable.
///
/// This is an addressing restriction, not a sandbox: `bash` in a local
/// workspace can still `cd` anywhere on the machine (see the workspace
/// plugin's caveat), and absolute paths are allowed inside a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSet {
    /// Workspace names, in order (`@rumu` and `@scratch` implicit).
    pub names: Vec<String>,
}

impl Structured for WorkspaceSet {
    const TAG: &'static str = "workspace.set";
    const V: u32 = 1;
}

/// One entry of the branch's card mount table: a deck mounted at `/cards/…`.
/// Mounting makes a deck readable; writing to it is approved separately
/// (the session's own deck is always writable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CardMount {
    /// The mounted deck's id.
    pub deck: String,
    /// The mount point, always under `/cards` (`/cards/self`, `/cards/user`).
    pub at: String,
}

impl Structured for CardMount {
    const TAG: &'static str = "card.mount";
    const V: u32 = 1;
}

/// Payload of a card unmount marker (v1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardUnmount {
    pub at: String,
}

impl Structured for CardUnmount {
    const TAG: &'static str = "card.unmount";
    const V: u32 = 1;
}

/// Payload of the session `root` marker (v1). The assistant lives on the root so
/// that every path of the session carries it, including sub-agents forked at
/// the root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRoot {
    /// The deck id of the assistant the session runs as.
    pub assistant: String,
    /// The assistant deck whose `agent` call this session works for. Every
    /// path carries it: a delegated session hands nothing further on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
    /// The deck's `deck.yaml` mounts, seeded at session start (the session's
    /// own deck is implicit at `/cards/self`), and the project the session
    /// starts in at [`crate::PROJECT_MOUNT`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<CardMount>,
}

impl Structured for SessionRoot {
    const TAG: &'static str = "root";
    const V: u32 = 1;
}

/// Payload of the `turn.error` marker (v1): a turn that failed terminally
/// after its retries ran out. Recorded at the branch head as a durable record
/// of the failure; the marker's empty text keeps it out of the model's
/// projection. The UI shows it as the error row while the reply is owed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct TurnError {
    pub message: String,
}

impl Structured for TurnError {
    const TAG: &'static str = "turn.error";
    const V: u32 = 1;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ToolCall {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

impl ToolCall {
    /// One model tool call.
    pub fn new(call_id: impl Into<String>, name: impl Into<String>, args: Value) -> Self {
        Self {
            call_id: call_id.into(),
            name: name.into(),
            args,
        }
    }
}

/// How the model call behind an Assistant node went, beside its tokens.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CallStats {
    /// Request sent → first streamed text, reasoning or tool-call chunk, ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    /// Wall time the model spent reasoning before its first text token, ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason_ms: Option<u64>,
    /// Size of the request as JSON, bytes: what went up the wire, give or
    /// take the protocol's own field names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_bytes: Option<u64>,
    /// Transient failures retried before this call succeeded.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Prompt tokens read from cache.
    pub cached_tokens: u64,
    /// Prompt tokens written to cache.
    pub cache_creation_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Branch {
    pub id: BranchId,
    pub session: SessionId,
    pub head: NodeId,
    /// Node this branch was created from (its first own node has this as parent).
    pub forked_from: Option<NodeId>,
    pub kind: BranchKind,
    pub status: BranchStatus,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum BranchKind {
    /// The session's one user-visible conversation line. Every edit or
    /// regenerate forks a new one and activates it; the branch it replaces
    /// stays reachable through the message variant switcher.
    Main,
    Agent {
        task: String,
        parent_branch: BranchId,
        /// The `agent` tool call that started this branch: the model's call
        /// id, or the internal call id (`"{parent_call}.{seq}"`) when issued
        /// from a plugin. The UI hangs the child branch off that call row.
        call_id: String,
        /// True when `call_id` is an internal call issued from a plugin
        /// (e.g. a `code` script) rather than a direct model call.
        #[serde(default)]
        via_plugin: bool,
        /// The resolved `inherit` argument the sub-agent was started with.
        #[serde(default)]
        inherit: Inherit,
    },
    /// A side question: the user's aside off `parent_branch` (a main line),
    /// forked at its last safe point. It sees the conversation up to there;
    /// the main line never sees it. No tools: every call is refused.
    Side {
        parent_branch: BranchId,
        /// The passage of the parent's conversation the question is about.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quote: Option<Quote>,
    },
}

/// A passage the user selected in a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Quote {
    /// The assistant node whose text holds it.
    pub node: NodeId,
    pub text: String,
}

/// How an `agent` sub-agent inherits from the branch that spawned it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct Inherit {
    #[serde(default)]
    pub context: InheritContext,
    /// `None` = every workspace of the parent; `Some(names)` attaches only
    /// those of them to the child path (a `workspace.set` marker).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<String>>,
}

/// Context inheritance for an `agent` sub-agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InheritContext {
    /// Fork at the assistant node containing the call: the child sees the
    /// whole conversation up to that point.
    #[default]
    Full,
    /// Fork at the session root: the child sees only its task.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BranchStatus {
    Active,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct SessionInfo {
    pub id: SessionId,
    pub title: String,
    pub active_branch: BranchId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// When the user last moved the session on: created it, sent on its main
    /// line, or moved it to another project. Session lists order by it, so
    /// the model's own activity does not reshuffle them. Derived by the store.
    #[serde(default)]
    pub touched_at: Timestamp,
    /// When the user last looked at the session: it has unseen updates while
    /// `updated_at` is later. `None` (never recorded) counts as seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
    /// The assistant the session runs as.
    pub assistant: String,
    /// The project deck of the active branch (the deck mounted at
    /// [`crate::PROJECT_MOUNT`]); `None` outside any project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
}

/// State derived purely from the root→head path (`state = f(path)`), merged
/// with the workspace registry by the kernel (see `rumu-kernel
/// state::merge_workspaces`): `workspaces` and `mounted` are then filled in
/// and `default_workspace` fully resolved.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct PathState {
    /// The workspaces the branch may address: every registered one, or those
    /// a `workspace.set` marker narrows it to. Filled in by the kernel's
    /// merge; empty straight out of `derive_state`.
    pub workspaces: Vec<Workspace>,
    /// The project's mounted workspaces among `workspaces`, in the project's
    /// order (the first is its default). Filled in by the kernel's merge.
    #[serde(default)]
    pub mounted: Vec<String>,
    /// The last `workspace.set` marker's names (a sub-agent's
    /// `inherit.workspaces`); `None` addresses every registered workspace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub restrict: Option<Vec<String>>,
    /// The project deck: the one mounted at [`crate::PROJECT_MOUNT`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project: Option<String>,
    /// Default workspace name: the last `workspace.use` marker here; after
    /// the kernel's merge, resolved (marker → the project's first → `@scratch`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_workspace: Option<String>,
    /// The assistant from the session's root.
    pub assistant: String,
    /// The assistant whose `agent` call this session works for, from the
    /// session's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<String>,
    /// The branch's card mount table (root defaults + `deck.mount` markers).
    #[serde(default)]
    pub mounts: Vec<CardMount>,
    /// The last `model.use` marker: unset (or after a `model.reset`) runs
    /// on the assistant's model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<crate::ModelUse>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn call_stats_sit_flat_on_the_assistant_body() {
        let old = json!({"type": "assistant", "content": {"text": "hi"}, "reason_ms": 40,
            "tool_calls": [], "model": "p/m", "usage": Usage::default()});
        let body: NodeBody = serde_json::from_value(old).unwrap();
        let NodeBody::Assistant { stats, .. } = &body else {
            panic!("not an assistant body")
        };
        assert_eq!(stats.reason_ms, Some(40));
        assert_eq!((stats.ttft_ms, stats.retries), (None, 0));

        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["reason_ms"], 40);
        assert!(v.get("ttft_ms").is_none() && v.get("retries").is_none());

        let stats = CallStats {
            ttft_ms: Some(900),
            reason_ms: None,
            request_bytes: None,
            retries: 2,
        };
        let v = json!({"type": "assistant", "content": {"text": ""}, "ttft_ms": 900,
            "retries": 2, "tool_calls": [], "model": "p/m", "usage": Usage::default()});
        let NodeBody::Assistant { stats: read, .. } = serde_json::from_value(v).unwrap() else {
            panic!("not an assistant body")
        };
        assert_eq!(read, stats);
    }

    fn image(name: &str) -> Attachment {
        Attachment {
            kind: AttachmentKind::Image,
            hash: "0".repeat(64),
            mime: "image/png".into(),
            name: Some(name.into()),
            width: 4,
            height: 3,
            size: 10,
        }
    }

    fn content(text: &str, names: &[&str]) -> Content {
        Content::text(text).with_attachments(names.iter().map(|n| image(n)))
    }

    /// Segments as a compact string: text verbatim, images as `<n>` when
    /// anchored and `<n…>` when appended.
    fn shape(c: &Content) -> String {
        c.segments()
            .iter()
            .map(|s| match s {
                Segment::Text(t) => (*t).to_owned(),
                Segment::Image { n, anchored, .. } => {
                    format!("<{n}{}>", if *anchored { "" } else { "…" })
                }
            })
            .collect()
    }

    #[test]
    fn anchors_place_images_inline() {
        let c = content("before [Image #2] and [Image #1] after", &["a", "b"]);
        assert_eq!(shape(&c), "before <2> and <1> after");
        // Adjacent anchors and anchors at the edges leave no empty text.
        let c = content("[Image #1][Image #2]", &["a", "b"]);
        assert_eq!(c.segments().len(), 2);
    }

    #[test]
    fn stray_anchors_stay_text_and_missing_ones_append() {
        let c = content(
            "[Image #1] again [Image #1], none [Image #3], [Image #01] [Image #] [Image #2",
            &["a", "b"],
        );
        assert_eq!(
            shape(&c),
            "<1> again [Image #1], none [Image #3], [Image #01] [Image #] [Image #2<2…>"
        );
        // No anchors at all: text first, then every image.
        assert_eq!(shape(&content("look", &["a", "b"])), "look<1…><2…>");
        assert_eq!(shape(&content("", &["a"])), "<1…>");
        // Text without attachments is one segment, anchors included.
        assert_eq!(shape(&content("[Image #1]", &[])), "[Image #1]");
    }

    #[test]
    fn rendered_uses_placeholders() {
        let c = content("see [Image #1]:", &["a.png", "b.png"]);
        assert_eq!(c.rendered(), "see [image: a.png 4×3]:\n[image: b.png 4×3]");
        assert_eq!(content("plain", &[]).rendered(), "plain");
    }
}
