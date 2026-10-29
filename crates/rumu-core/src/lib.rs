//! rumu-core: the shared contract between the kernel, plugins and the wire.
//!
//! Pure data types, address normalization and traits only — no runtime
//! behaviour lives here; the kernel implements the semantics on top of
//! these.

mod addr;
mod code;
mod error;
mod event;
mod ids;
mod index;
mod node;
mod plugin;
mod schema;
mod task;
mod workspace;

pub use addr::*;
pub use code::*;
pub use error::*;
pub use event::*;
pub use ids::*;
pub use index::*;
pub use node::*;
pub use plugin::*;
pub use schema::*;
pub use task::*;
pub use workspace::*;

/// Runtime-typed paths (workspace roots and machine addresses) and
/// platform-independent POSIX paths (the `@rumu` virtual workspace).
/// Re-exported so the other crates share one version.
pub use typed_path::{PathType, Utf8TypedPath, Utf8TypedPathBuf, Utf8UnixPath, Utf8UnixPathBuf};

/// Milliseconds since the Unix epoch.
pub type Timestamp = i64;

pub fn now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Character cap applied automatically to every tool output before the model
/// sees it ([`ToolOutput::cap`]).
pub const OUTPUT_LIMIT: usize = 20_000;

/// Truncate `text` to [`OUTPUT_LIMIT`] characters, appending a marker that
/// says how much was dropped.
pub fn truncate_output(text: String) -> String {
    truncate_chars(text, OUTPUT_LIMIT)
}

/// [`truncate_output`] with an explicit character limit.
pub fn truncate_chars(text: String, limit: usize) -> String {
    let total = text.chars().count();
    if total <= limit {
        return text;
    }
    let head: String = text.chars().take(limit).collect();
    format!("{head}\n… [truncated, {} of {total} chars]", total - limit)
}
