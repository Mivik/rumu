//! Card usage records derived from a branch path.
//!
//! Every way a card's body reaches the model leaves a trace on the path.
//! Cards are keyed by their deck id and deck-local path:
//!
//! - auto-import: `Note` with `content.data = memory.import {deck, path, rev}`
//!   (and the card text) — the payload carries the revision, so imports are
//!   per-version;
//! - tool read/write: a `Note` with empty text and `content.data = card_use
//!   {deck, path, rev, view_rev, call_id, op}` appended right after the tool
//!   batch —
//!   counted only when a successful ToolResult with the same `call_id` exists
//!   on the path (aborted calls leave stale records behind). Calls a `code`
//!   script made through `Cx::invoke` are ordinary ToolResult nodes too, so
//!   the same rule applies — a successful invoke counts even if the script
//!   throws afterwards;
//!
//! The latest use per card wins. This is the single source for the update
//! notice hook and the web "On this path" view; it is mirrored in
//! `web/app/lib/cards.ts`.

use std::collections::{BTreeMap, HashSet};

use rumu_core::{Content, Node, NodeBody, Structured};

/// One recorded use of a card on a path. `rev` is the field the memory
/// plugin needs (update notices + import dedup); `view_rev` is what the edit
/// tools need (read-before-write concurrency — absent on import notes, which
/// carry no editable view).
#[derive(Debug, Clone)]
pub struct Use {
    pub rev: Option<String>,
    pub view_rev: Option<String>,
    pub commit: Option<String>,
}

/// What a `card_use` record describes: a tool read, write or edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum CardOp {
    Read,
    Write,
    Edit,
}

/// Payload of the `card_use` marker (v1), recorded by card reads/writes
/// (`read`/`write`/`edit` on `@rumu:/cards/<mount>/<path>`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CardUse {
    /// The id of the deck holding the card.
    pub deck: String,
    /// The card's path within that deck.
    pub path: String,
    pub rev: String,
    pub view_rev: String,
    /// The deck `HEAD` commit at read time (CAS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default = "default_op")]
    pub op: CardOp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
}

fn default_op() -> CardOp {
    CardOp::Read
}

impl Structured for CardUse {
    const TAG: &'static str = "card_use";
    const V: u32 = 1;
}

/// Marker tag recorded by card reads/writes.
pub const CARD_USE: &str = CardUse::TAG;

/// Build the `card_use` marker content for a read/write.
pub fn card_use(
    deck: &str,
    path: &str,
    rev: &str,
    view_rev: &str,
    commit: Option<&str>,
    op: CardOp,
    call_id: Option<&str>,
) -> Content {
    Content::marker(&CardUse {
        deck: deck.to_owned(),
        path: path.to_owned(),
        rev: rev.to_owned(),
        view_rev: view_rev.to_owned(),
        commit: commit.map(str::to_owned),
        op,
        call_id: call_id.map(str::to_owned),
    })
}

/// Payload of an auto-import note (`memory.import` v1).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CardImport {
    pub deck: String,
    pub path: String,
    pub rev: String,
}

impl Structured for CardImport {
    const TAG: &'static str = "memory.import";
    const V: u32 = 1;
}

/// Payload of a card update notice (`memory.card_update` v1). `rev` is the
/// current revision, or `"deleted"`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct CardUpdate {
    pub deck: String,
    pub path: String,
    pub rev: String,
}

impl Structured for CardUpdate {
    const TAG: &'static str = "memory.card_update";
    const V: u32 = 1;
}

/// The call ids that produced a successful `ToolResult` on the path.
pub(crate) fn ok_calls(nodes: &[Node]) -> HashSet<&str> {
    nodes
        .iter()
        .filter_map(|n| match &n.body {
            NodeBody::ToolResult {
                call_id,
                is_error: false,
                ..
            } => Some(call_id.as_str()),
            _ => None,
        })
        .collect()
}

/// The latest `card_use` marker record of `(deck, path)` on the path
/// (validated by a successful ToolResult). Import notes are ignored — the
/// read-before-write check of the edit tools needs the tool read's `view_rev`,
/// which only tool uses carry.
pub fn latest_tool_use(nodes: &[Node], deck: &str, path: &str) -> Option<Use> {
    uses_on_path(nodes)
        .remove(&(deck.to_owned(), path.to_owned()))
        .filter(|u| u.view_rev.is_some())
}

/// Build the `(deck, path) → latest Use` map for a root→head path.
pub fn uses_on_path(nodes: &[Node]) -> BTreeMap<(String, String), Use> {
    fold_uses(nodes, &ok_calls(nodes))
}

/// One pass over the path: imports and `card_use` records, latest wins.
/// `ok_calls` validates the tool uses.
fn fold_uses(nodes: &[Node], ok_calls: &HashSet<&str>) -> BTreeMap<(String, String), Use> {
    let mut uses: BTreeMap<(String, String), Use> = BTreeMap::new();
    for n in nodes {
        let NodeBody::Note { content, .. } = &n.body else {
            continue;
        };
        if let Some(import) = content.decode::<CardImport>() {
            uses.insert(
                (import.deck, import.path),
                Use {
                    rev: Some(import.rev),
                    view_rev: None,
                    commit: None,
                },
            );
            continue;
        }
        let Some(u) = content.decode::<CardUse>() else {
            continue;
        };
        if !u.call_id.as_deref().is_none_or(|c| ok_calls.contains(c)) {
            continue; // aborted/unanswered call: not a real use
        }
        uses.insert(
            (u.deck, u.path),
            Use {
                rev: Some(u.rev),
                view_rev: Some(u.view_rev),
                commit: u.commit,
            },
        );
    }
    uses
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{NodeId, now};
    use serde_json::json;

    fn import(path: &str, rev: &str, text: &str) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: Default::default(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "memory".into(),
                content: Content::note_with(
                    text,
                    &CardImport {
                        deck: "d".into(),
                        path: path.into(),
                        rev: rev.into(),
                    },
                ),
            },
        }
    }

    fn use_note(data: serde_json::Value) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: Default::default(),
            at: now(),
            dur_ms: None,
            body: NodeBody::Note {
                source: "workspace".into(),
                content: Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(rumu_core::Tagged {
                        tag: CARD_USE.into(),
                        v: 1,
                        data,
                    }),
                },
            },
        }
    }

    fn result(call_id: &str, is_error: bool) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: Default::default(),
            at: now(),
            dur_ms: None,
            body: NodeBody::ToolResult {
                call_id: call_id.into(),
                content: rumu_core::Content::text("out"),
                is_error,
                invoke: None,
            },
        }
    }

    #[test]
    fn card_payloads_round_trip() {
        let import = CardImport {
            deck: "d".into(),
            path: "dev/x".into(),
            rev: "a1b2c3d4".into(),
        };
        let content = Content::note_with("body", &import);
        assert_eq!(content.decode::<CardImport>(), Some(import));
        assert_eq!(
            Content::marker(&CardUpdate {
                deck: "d".into(),
                path: "dev/x".into(),
                rev: "deleted".into(),
            })
            .decode::<CardUpdate>()
            .unwrap()
            .rev,
            "deleted"
        );
    }

    #[test]
    fn uses_on_path_latest_wins() {
        let nodes = vec![
            import("x", "a1b2c3d4", "# x\nv1"),
            import("y", "1234abcd", "# y\nimported"),
            result("c1", false),
            result("c2", true),
            use_note(
                json!({"deck": "d", "path": "z", "rev": "ff00ff00", "view_rev": "00ff00ff", "op": "read", "call_id": "c1"}),
            ),
            use_note(
                json!({"deck": "d", "path": "w", "rev": "ee00ee00", "view_rev": "00ee00ee", "op": "read", "call_id": "c2"}),
            ), // error result
            use_note(
                json!({"deck": "d", "path": "v", "rev": "dd00dd00", "view_rev": "00dd00dd", "op": "read", "call_id": "c9"}),
            ), // no result
            use_note(
                json!({"deck": "d", "path": "u", "rev": "cc00cc00", "op": "read", "call_id": "c1"}),
            ), // no view_rev
            import("x", "e5f6a7b8", "# x\nv2"),
        ];
        let uses = uses_on_path(&nodes);
        assert_eq!(uses.len(), 3, "w, v and u are dropped: {uses:?}");
        let key = |p: &str| ("d".to_owned(), p.to_owned());
        assert!(uses[&key("x")].view_rev.is_none()); // import
        assert_eq!(uses[&key("x")].rev.as_deref(), Some("e5f6a7b8")); // latest wins
        assert!(uses[&key("y")].view_rev.is_none()); // import
        assert_eq!(uses[&key("z")].rev.as_deref(), Some("ff00ff00"));
        assert_eq!(uses[&key("z")].view_rev.as_deref(), Some("00ff00ff"));

        // latest_tool_use ignores import notes and is identity-scoped.
        assert_eq!(
            latest_tool_use(&nodes, "d", "z")
                .unwrap()
                .view_rev
                .as_deref(),
            Some("00ff00ff")
        );
        assert!(latest_tool_use(&nodes, "d", "x").is_none());
        let only_note = vec![import("x", "a1b2c3d4", "# x\nv1")];
        assert!(latest_tool_use(&only_note, "d", "x").is_none());
    }

    #[test]
    fn invoke_calls_use_their_own_result() {
        let edit = |path: &str, call: &str| {
            use_note(
                json!({"deck": "d", "path": path, "rev": "ff00ff00", "view_rev": "00ff00ff", "op": "edit", "call_id": call}),
            )
        };
        let nodes = vec![
            edit("a", "code1.1"),
            result("code1.1", false), // the invoke succeeded…
            result("code1", true),    // …even though the script threw afterwards
            edit("b", "code2.1"),     // no result on the path
            edit("c", "code2"),       // the failed parent itself
        ];
        let uses = uses_on_path(&nodes);
        let paths: Vec<&str> = uses.keys().map(|(_, p)| p.as_str()).collect();
        assert_eq!(paths, ["a"]);
    }
}
