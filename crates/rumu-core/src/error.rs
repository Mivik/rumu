//! The shared error types.
//!
//! [`Error`] is the error used at the crate's trait boundaries
//! ([`crate::Plugin`], [`crate::Runtime`], [`crate::History`],
//! [`crate::Inbox`], [`crate::Cx`]), whose implementations live in other
//! crates. Each implementation crate converts its own typed error into
//! [`Error::Other`] via `Error::other`, preserving the display/source chain.
//! [`IoError`] is the shared filesystem/process failure shape used by the
//! storage and plugin crates, so error context ("reading /x: permission
//! denied") is structured.

use std::fmt;
use std::path::Path;

/// A result with the shared boundary error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A filesystem, process or other I/O operation that failed.
///
/// `subject` is the path (or other object) the operation was applied to;
/// `None` when the operation has no path (resolving the current directory).
#[derive(Debug)]
pub struct IoError {
    /// What was being done, e.g. `reading`, `writing`, `spawning`.
    pub op: &'static str,
    /// What it was done to, e.g. a path.
    pub subject: Option<String>,
    /// The underlying I/O failure.
    pub source: std::io::Error,
}

impl IoError {
    /// Build an error with an explicit subject.
    pub fn new(op: &'static str, subject: impl Into<String>, source: std::io::Error) -> Self {
        Self {
            op,
            subject: Some(subject.into()),
            source,
        }
    }

    /// Build an error for an operation without a path-like subject.
    pub fn op(op: &'static str, source: std::io::Error) -> Self {
        Self {
            op,
            subject: None,
            source,
        }
    }

    /// A lazy constructor for `map_err`: the subject is formatted only when
    /// an error actually occurs.
    pub fn at<'a>(op: &'static str, path: &'a Path) -> impl FnOnce(std::io::Error) -> Self + 'a {
        move |source| Self::new(op, path.display().to_string(), source)
    }

    /// The underlying I/O error kind.
    pub fn kind(&self) -> std::io::ErrorKind {
        self.source.kind()
    }

    /// True when the failure was "not found".
    pub fn is_not_found(&self) -> bool {
        self.kind() == std::io::ErrorKind::NotFound
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.subject {
            Some(subject) => write!(f, "{} {}: {}", self.op, subject, self.source),
            None => write!(f, "{}: {}", self.op, self.source),
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The error type at the crate's trait boundaries.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No default workspace is set and an unqualified path was given.
    #[error(
        "no default workspace is set — `list @rumu:/workspaces` lists them; `invoke` \
         `workspace.use` on one to pick it"
    )]
    NoDefaultWorkspace,

    /// A `<name>:<path>` qualifier names an unknown workspace.
    #[error(
        "unknown workspace `{name}` (available: {available}); to address a file literally \
         named `{raw}`, write `./{raw}`"
    )]
    UnknownWorkspace {
        name: String,
        raw: String,
        available: String,
    },

    /// `~` cannot be expanded on an ssh workspace.
    #[error("`~` is not expanded on ssh workspaces; use an absolute path")]
    TildeOnSsh,

    /// A path contains a NUL byte.
    #[error("invalid path")]
    InvalidPath,

    /// `Cx::invoke` was called outside a tool call.
    #[error("invoke requires a call context (not available here)")]
    NoCallContext,

    /// `Cx::invoke` was called from a background task.
    #[error("invoke is not available in a background task")]
    InvokeInBackground,

    /// A note carries neither text nor a structured payload.
    #[error("note has neither text nor data")]
    EmptyNote,

    /// A plain message for boundary implementations and test fakes.
    #[error("{0}")]
    Message(String),

    /// A typed error from another crate, carried through the boundary.
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    /// A plain message error, for boundary implementations and test fakes.
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Carry another crate's typed error through the boundary.
    pub fn other<E>(error: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }
}

/// The unknown-workspace message used by address resolution.
pub(crate) fn unknown_workspace_error(
    name: &str,
    raw: &str,
    workspaces: &[crate::Workspace],
) -> Error {
    let mut names: Vec<&str> = workspaces.iter().map(|w| w.name.as_str()).collect();
    names.push(crate::RUMU_WORKSPACE);
    Error::UnknownWorkspace {
        name: name.to_owned(),
        raw: raw.to_owned(),
        available: names.join(", "),
    }
}
