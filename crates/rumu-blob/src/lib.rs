//! rumu-blob: the content-addressed blob store behind image attachments.
//!
//! A blob is a file `<dir>/<sha256-hex>` whose bytes never change once
//! written, so a node's [`Attachment`] names it by hash and the model
//! projection of that node stays byte-identical (the cache contract). Images
//! are normalized once, at ingest ([`Blobs::ingest_image`]); nothing
//! downstream re-encodes them.
//!
//! Reference tracking is split in two: the session store knows which hashes
//! its nodes reference, and [`Blobs::gc`] deletes every other blob once it is
//! older than a grace period — an upload stays unreferenced until the prompt
//! that carries it is written, and the grace period covers that gap.

mod error;
mod normalize;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use rumu_core::{Attachment, IoError};
use sha2::{Digest, Sha256};
use tracing::debug;

pub use error::{Error, Result};
pub use normalize::{MAX_EDGE, MAX_INPUT_BYTES, MAX_STORED_BYTES};

/// Distinguishes concurrent temp files of one process.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A directory of content-addressed blobs. Cheap to clone; every handle on
/// the same directory sees the same blobs.
#[derive(Debug, Clone)]
pub struct Blobs {
    dir: PathBuf,
}

impl Blobs {
    /// Open the blob store of a `RUMU_HOME`: `<home>/blobs`. Every component
    /// (server, plugins, LLM adapter) opens its own handle on it.
    pub fn in_home(home: &Path) -> Result<Self> {
        Self::open(&home.join("blobs"))
    }

    /// Open (creating if needed) the blob directory.
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).map_err(IoError::at("creating blob dir", dir))?;
        Ok(Self {
            dir: dir.to_owned(),
        })
    }

    /// Normalize an uploaded or tool-read image (PNG/JPEG/GIF/WebP in;
    /// orientation applied, long edge ≤ [`MAX_EDGE`], at most
    /// [`MAX_STORED_BYTES`] out) and store the result. CPU-bound: call it off
    /// the async executor.
    pub fn ingest_image(&self, bytes: &[u8], name: Option<String>) -> Result<Attachment> {
        let image = normalize::normalize(bytes)?;
        let hash = self.put(&image.bytes)?;
        Ok(Attachment {
            kind: rumu_core::AttachmentKind::Image,
            hash,
            mime: image.mime.to_owned(),
            name,
            width: image.width,
            height: image.height,
            size: image.bytes.len() as u64,
        })
    }

    /// Store `bytes` as-is and return their hash. Storing bytes that already
    /// exist refreshes the blob's age, so a re-uploaded orphan gets a full
    /// grace period again.
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let hash = hash_of(bytes);
        let path = self.dir.join(&hash);
        if path.exists() {
            fs::File::options()
                .append(true)
                .open(&path)
                .and_then(|f| f.set_modified(SystemTime::now()))
                .map_err(IoError::at("touching blob", &path))?;
            return Ok(hash);
        }
        // Write-then-rename: a reader never sees a partial blob.
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!(".{hash}.{}.{seq}.tmp", std::process::id()));
        fs::write(&tmp, bytes).map_err(IoError::at("writing blob", &tmp))?;
        fs::rename(&tmp, &path).map_err(IoError::at("renaming blob", &path))?;
        Ok(hash)
    }

    /// The bytes of a blob.
    pub fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.path(hash)?;
        fs::read(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(hash.to_owned()),
            _ => IoError::new("reading blob", path.display().to_string(), e).into(),
        })
    }

    /// The file a blob lives in (it may not exist). Rejects anything that is
    /// not a lowercase hex SHA-256, so a hash from the wire can never name a
    /// path outside the blob directory.
    pub fn path(&self, hash: &str) -> Result<PathBuf> {
        if !is_hash(hash) {
            return Err(Error::InvalidHash(hash.to_owned()));
        }
        Ok(self.dir.join(hash))
    }

    /// Whether a blob exists.
    pub fn contains(&self, hash: &str) -> bool {
        self.path(hash).is_ok_and(|p| p.is_file())
    }

    /// The [`Attachment`] for a stored image, rebuilt from the blob itself
    /// (format, size and dimensions), so a reference from the wire is
    /// trusted for its hash and display name only.
    pub fn describe(&self, hash: &str, name: Option<String>) -> Result<Attachment> {
        let bytes = self.get(hash)?;
        let (mime, width, height) = normalize::probe(&bytes)?;
        Ok(Attachment {
            kind: rumu_core::AttachmentKind::Image,
            hash: hash.to_owned(),
            mime: mime.to_owned(),
            name,
            width,
            height,
            size: bytes.len() as u64,
        })
    }

    /// Delete every blob not in `live` whose last write is older than
    /// `grace`, plus temp files of interrupted writes as old. Returns how
    /// many files were removed.
    pub fn gc(&self, live: &HashSet<String>, grace: Duration) -> Result<usize> {
        let cutoff = SystemTime::now()
            .checked_sub(grace)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let entries =
            fs::read_dir(&self.dir).map_err(IoError::at("reading blob dir", &self.dir))?;
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let orphan = if is_hash(name) {
                !live.contains(name)
            } else {
                name.starts_with('.') && name.ends_with(".tmp")
            };
            if !orphan {
                continue;
            }
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t < cutoff);
            if old {
                let path = entry.path();
                fs::remove_file(&path).map_err(IoError::at("removing blob", &path))?;
                debug!(blob = name, "removed unreferenced blob");
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Whether `bytes` start like an image [`Blobs::ingest_image`] accepts (PNG,
/// JPEG, GIF, WebP). A cheap magic-byte sniff; decoding may still fail.
pub fn is_image(bytes: &[u8]) -> bool {
    normalize::accepted_format(bytes).is_some()
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn hash_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests;
