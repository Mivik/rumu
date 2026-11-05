//! The card crate's typed errors.

use std::path::PathBuf;

use rumu_core::IoError;

/// A deck or card failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation under the card root failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// The model tried to write through a link card.
    #[error("card `{id}` is a link; write its target instead")]
    LinkWrite { id: String },

    /// A link card cannot point at another link card.
    #[error("card `{target}` is itself a link; links are one level deep")]
    LinkToLink { target: String },

    /// A card path is not valid (see `valid_card_path`).
    #[error("invalid card path `{id}`")]
    InvalidPath { id: String },

    /// The card changed since the write's base revision was read.
    #[error("card `{id}` changed since it was read; read it again")]
    Stale { id: String },

    /// Other cards link to this card, so it cannot become a link itself.
    #[error("card `{id}` is a link target; links are one level deep")]
    LinkedTarget { id: String },

    /// A deck file is malformed.
    #[error("{path}: {message}")]
    Config { path: PathBuf, message: String },

    /// A git repository operation failed.
    #[error("git: {message}")]
    Git { message: String },
}

/// A result with the card error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for rumu_core::Error {
    fn from(error: Error) -> Self {
        rumu_core::Error::other(error)
    }
}

impl From<Error> for rumu_core::ToolError {
    fn from(error: Error) -> Self {
        Self::Fatal(error.into())
    }
}

/// A malformed editable card view (the post-validator of `write` / `edit`).
#[derive(Debug, thiserror::Error)]
pub enum CardViewError {
    /// The frontmatter is not valid YAML (this includes non-string keys,
    /// which a YAML-to-JSON conversion cannot represent).
    #[error("frontmatter is not valid YAML: {0}")]
    InvalidYaml(serde_saphyr::Error),

    /// The frontmatter is not a mapping.
    #[error("frontmatter must be a YAML mapping (key: value lines)")]
    NotMapping,

    /// A known field has the wrong type.
    #[error("`{field}` must be a string")]
    FieldNotString { field: String },
}
