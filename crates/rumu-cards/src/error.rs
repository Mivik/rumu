//! The card crate's typed errors.

use std::path::PathBuf;

use rumu_core::IoError;

/// A card-library failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation under the card root failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// A card path tried to leave the library root.
    #[error("card path escapes the card root: {id}")]
    EscapesRoot { id: String },

    /// A write would go through a symlink.
    #[error("refusing to write through a symlink: {path}")]
    Symlink { path: PathBuf },

    /// A card id is not a valid path id.
    #[error("invalid card id `{id}`")]
    InvalidId { id: String },
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
