//! Per-deck card use scores: `usage.json` beside the deck repository.
//!
//! The file lives in the deck's directory (a bare repository), never in its
//! tree, so it is neither committed nor synced. Per card identity it stores a
//! score and the time the score was last updated. A write bumps it by
//! [`COMMIT_WEIGHT`], a successful model read by [`READ_WEIGHT`]; ranking
//! decays the stored score to now with a 30-day half-life.
//!
//! A missing or unreadable file means every card is at zero. Writes are
//! read-modify-write under one process-wide lock and land atomically.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rumu_core::Timestamp;
use serde::{Deserialize, Serialize};

/// The index file, in the deck's repository directory.
pub const USAGE_FILE: &str = "usage.json";
/// Half-life of a score: 30 days in ms.
pub const HALF_LIFE_MS: f64 = 30.0 * 86_400_000.0;
/// A commit touching a card.
pub const COMMIT_WEIGHT: f64 = 3.0;
/// A successful model read of a card.
pub const READ_WEIGHT: f64 = 1.0;

/// One card's stored score and when it was last updated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct Entry {
    score: f64,
    at: Timestamp,
}

/// Serializes every read-modify-write, so two commits cannot lose a bump.
static LOCK: Mutex<()> = Mutex::new(());

fn path_in(dir: &Path) -> PathBuf {
    dir.join(USAGE_FILE)
}

fn read(dir: &Path) -> BTreeMap<String, Entry> {
    std::fs::read_to_string(path_in(dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Decay one stored score to `now_ms`.
fn effective(entry: &Entry, now_ms: Timestamp) -> f64 {
    let delta = (now_ms - entry.at).max(0) as f64;
    entry.score * 2.0_f64.powf(-delta / HALF_LIFE_MS)
}

/// Effective scores of every card, keyed by identity, at `now_ms`.
pub fn scores(dir: &Path, now_ms: Timestamp) -> BTreeMap<String, f64> {
    read(dir)
        .iter()
        .map(|(id, entry)| (id.clone(), effective(entry, now_ms)))
        .collect()
}

/// Add `weight` to one card's score and stamp it with `now_ms`.
pub fn bump(dir: &Path, id: &str, weight: f64, now_ms: Timestamp) {
    with_index(dir, |index| {
        let entry = index.entry(id.to_owned()).or_insert(Entry {
            score: 0.0,
            at: now_ms,
        });
        entry.score = effective(entry, now_ms) + weight;
        entry.at = now_ms;
    });
}

/// Apply one commit: drop the identities it removed, bump the ones it wrote.
/// An identity present in both lists was moved within the deck and keeps its
/// score (plus the commit's bump).
pub fn record_commit(dir: &Path, written: &[String], removed: &[String], now_ms: Timestamp) {
    if written.is_empty() && removed.is_empty() {
        return;
    }
    with_index(dir, |index| {
        for id in removed {
            if !written.contains(id) {
                index.remove(id);
            }
        }
        for id in written {
            let entry = index.entry(id.clone()).or_insert(Entry {
                score: 0.0,
                at: now_ms,
            });
            entry.score = effective(entry, now_ms) + COMMIT_WEIGHT;
            entry.at = now_ms;
        }
    });
}

/// Read, change, and atomically replace the index. An unreadable file reads
/// as empty and is replaced.
fn with_index(dir: &Path, change: impl FnOnce(&mut BTreeMap<String, Entry>)) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut index = read(dir);
    change(&mut index);
    let text = match serde_json::to_string(&index) {
        Ok(text) => text,
        Err(_) => return,
    };
    let target = path_in(dir);
    let temp = dir.join(format!(".{USAGE_FILE}.{}.tmp", std::process::id()));
    if std::fs::write(&temp, text).is_ok() && std::fs::rename(&temp, &target).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_decays_and_survives_moves() {
        let dir = tempfile::tempdir().unwrap();
        let now = 100 * HALF_LIFE_MS as Timestamp;
        bump(dir.path(), "a", COMMIT_WEIGHT, now);
        assert_eq!(scores(dir.path(), now)["a"], COMMIT_WEIGHT);

        // One half-life later the stored score is halved.
        let later = now + HALF_LIFE_MS as Timestamp;
        assert_eq!(scores(dir.path(), later)["a"], COMMIT_WEIGHT / 2.0);

        // A move is one write and one removal of the same identity.
        record_commit(dir.path(), &["a".into()], &["a".into()], later);
        // The move keeps the decayed score and bumps it by a commit.
        assert_eq!(
            scores(dir.path(), later)["a"],
            COMMIT_WEIGHT / 2.0 + COMMIT_WEIGHT
        );

        // A deletion drops the identity.
        record_commit(dir.path(), &[], &["a".into()], later);
        assert!(scores(dir.path(), later).is_empty());
    }

    #[test]
    fn missing_file_is_all_zeros() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scores(dir.path(), 1).is_empty());
    }
}
