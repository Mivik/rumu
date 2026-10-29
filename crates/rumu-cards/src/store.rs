//! Card storage: the byte-level backend of the card library. The home
//! library is the local filesystem under `RUMU_HOME/cards` (with
//! symlink/escape guards). All card-shaped logic above the bytes — parsing,
//! views, revisions, discovery — lives in [`crate::library`] and works over
//! [`FsCardStore`].

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rumu_core::IoError;
use tracing::warn;

use crate::card::valid_card_id;
use crate::{Error, Result};

/// The home library on the local filesystem: `cards/<path>.md` under
/// RUMU_HOME, loaded recursively. Symlinks anywhere inside the tree are
/// skipped (they could escape the card root), and writes refuse to leave the
/// canonicalized root or go through a symlink.
pub struct FsCardStore {
    pub root: PathBuf,
}

impl FsCardStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The home store: `<home>/cards`.
    pub fn home(home: &Path) -> Self {
        Self::new(home.join("cards"))
    }

    /// Recursively list card ids, sorted. The root itself may be a symlink.
    pub fn walk(&self) -> Result<Vec<String>> {
        if !self.root.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        self.visit(&self.root, "", &mut out)?;
        out.sort();
        Ok(out)
    }

    fn visit(&self, dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<()> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            // symlink_metadata: never follow symlinks inside the tree.
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                warn!(
                    path = %entry.path().display(),
                    "skipping symlink in card library"
                );
                continue;
            }
            let child_rel = if rel.is_empty() {
                name.to_owned()
            } else {
                format!("{rel}/{name}")
            };
            if ft.is_dir() {
                self.visit(&entry.path(), &child_rel, out)?;
            } else if ft.is_file() && name.ends_with(".md") {
                let id = child_rel.trim_end_matches(".md").to_owned();
                if valid_card_id(&id) {
                    out.push(id);
                } else {
                    warn!(id = %id, "skipping card with invalid id");
                }
            }
        }
        Ok(())
    }

    /// The card file's text, or None when it is missing / not a regular
    /// file (symlinked targets are refused).
    pub fn read(&self, id: &str) -> Result<Option<String>> {
        let path = self.root.join(format!("{id}.md"));
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        if !meta.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path).map_err(IoError::at("reading", &path))?;
        Ok(Some(text))
    }

    /// Write a card file, creating parent directories. Guards against
    /// escapes: the parent directory (after canonicalization) must stay under
    /// the canonicalized root, and the target must not be a symlink.
    pub fn write(&self, id: &str, content: &str) -> Result<()> {
        let path = self.root.join(format!("{id}.md"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(IoError::at("creating", parent))?;
            let canon_root = self
                .root
                .canonicalize()
                .map_err(IoError::at("canonicalizing card root", &self.root))?;
            let canon_parent = parent
                .canonicalize()
                .map_err(IoError::at("canonicalizing", parent))?;
            if !canon_parent.starts_with(&canon_root) {
                return Err(Error::EscapesRoot { id: id.to_owned() });
            }
        }
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_symlink()) {
            return Err(Error::Symlink { path });
        }
        // Atomic replace: write beside the target, then rename over it.
        let parent = path.parent().unwrap_or(&self.root);
        let mut tmp =
            tempfile::NamedTempFile::new_in(parent).map_err(IoError::at("writing", &path))?;
        tmp.write_all(content.as_bytes())
            .map_err(IoError::at("writing", &path))?;
        tmp.persist(&path)
            .map_err(|e| IoError::at("writing", &path)(e.error))?;
        Ok(())
    }

    /// File modification time in ms since Unix epoch, if available.
    pub fn mtime(&self, id: &str) -> Result<Option<rumu_core::Timestamp>> {
        let path = self.root.join(format!("{id}.md"));
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return Ok(None);
        };
        let Ok(modified) = meta.modified() else {
            return Ok(None);
        };
        Ok(modified
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_millis() as rumu_core::Timestamp))
    }
}
