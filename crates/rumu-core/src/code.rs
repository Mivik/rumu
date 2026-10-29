//! Records of tool calls made from inside a `code` script.
//!
//! Internal calls are not nodes: they never appear on the branch path and so
//! do not participate in state derivation. They are stored in a side table and
//! pushed to the UI as `code.call`, so a reloaded session can still show what a
//! script did.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BranchId, Timestamp};

/// One tool call issued by a `code` script, addressed by the enclosing
/// `code` tool call id plus a 1-based per-parent sequence number.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CodeCall {
    pub branch: BranchId,
    pub parent_call: String,
    pub seq: u32,
    /// The effective tool name (after any `tool.pre` rewrite).
    pub name: String,
    /// The effective args (after any `tool.pre` rewrite).
    pub args: Value,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
    /// The hook's block reason when `tool.pre` blocked the call.
    #[serde(default)]
    pub blocked: Option<String>,
    pub dur_ms: u64,
    pub at: Timestamp,
}
