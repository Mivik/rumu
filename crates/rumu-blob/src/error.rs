//! The blob crate's typed errors.

use rumu_core::IoError;

/// A blob-store or image-ingest failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation on the blob directory failed.
    #[error(transparent)]
    Io(#[from] IoError),

    /// Not a lowercase hex SHA-256.
    #[error("invalid blob hash {0:?}")]
    InvalidHash(String),

    /// No blob with this hash is stored.
    #[error("blob {0} not found")]
    NotFound(String),

    /// The bytes are not a PNG, JPEG, GIF or WebP image.
    #[error("unsupported image format (expected PNG, JPEG, GIF or WebP)")]
    Unsupported,

    /// The input is larger than ingest accepts.
    #[error("image is {size} bytes; the limit is {limit}")]
    TooLarge { size: usize, limit: usize },

    /// The image could not be decoded.
    #[error("decoding image: {0}")]
    Decode(String),

    /// The normalized image could not be encoded.
    #[error("encoding image: {0}")]
    Encode(String),
}

/// A result with the blob error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<Error> for rumu_core::Error {
    fn from(error: Error) -> Self {
        rumu_core::Error::other(error)
    }
}
