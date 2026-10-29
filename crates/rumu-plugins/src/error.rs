//! The plugin crate's typed errors.

use std::path::PathBuf;

use rumu_core::IoError;

use crate::stdio::StdioError;
use crate::workspace::BackendError;

/// A plugin-loading or plugin-runtime failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// The card library failed.
    #[error(transparent)]
    Cards(#[from] rumu_cards::Error),

    /// A kernel service (path, state, history, inbox) failed.
    #[error(transparent)]
    Core(#[from] rumu_core::Error),

    /// A workspace backend failed.
    #[error(transparent)]
    Backend(#[from] BackendError),

    /// The external-plugin transport failed.
    #[error(transparent)]
    Stdio(#[from] StdioError),

    /// A JSON value could not be produced or read.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// A YAML file could not be parsed. Boxed: `serde_saphyr::Error`
    /// carries a rendered snippet and is large.
    #[error("parsing {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: Box<serde_saphyr::Error>,
    },

    /// A config file entry is malformed.
    #[error("{path}: {message}")]
    Config { path: PathBuf, message: String },

    /// A workspace preset is invalid.
    #[error("workspace `{name}`: {message}")]
    WorkspaceConfig { name: String, message: String },

    /// An external plugin could not be loaded.
    #[error("loading external plugin `{name}`: {source}")]
    ExternalPlugin {
        name: String,
        #[source]
        source: StdioError,
    },
}

/// A result with the plugin error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for rumu_core::Error {
    fn from(error: Error) -> Self {
        rumu_core::Error::other(error)
    }
}

impl From<StdioError> for rumu_core::Error {
    fn from(error: StdioError) -> Self {
        rumu_core::Error::other(error)
    }
}
