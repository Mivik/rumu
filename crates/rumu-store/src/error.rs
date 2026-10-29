//! The store crate's typed errors.

use rumu_core::{IoError, SessionId};

/// A session-store failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation on the sessions directory or a file failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// A record could not be serialized for the journal.
    #[error("serializing a record of session {session}: {source}")]
    Serialize {
        session: SessionId,
        #[source]
        source: serde_json::Error,
    },

    /// A session with this id is already loaded.
    #[error("session {id} already exists")]
    SessionExists { id: SessionId },

    /// A record arrived before its session header.
    #[error("record before the session header")]
    RecordBeforeHeader,

    /// A session file could not be folded and every read of it fails.
    #[error("session {session} is unreadable: {message}")]
    Unreadable { session: SessionId, message: String },
}

/// A result with the store error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for rumu_core::Error {
    fn from(error: Error) -> Self {
        rumu_core::Error::other(error)
    }
}
