//! Catalog planning tests: budgeting, folding, Frecency and progressive
//! unfolding.

use std::collections::BTreeMap;

use rumu_core::{Content, Node, NodeBody, NodeId, Timestamp, WorkspaceUse};

use super::*;
use crate::card::parse_card;
use crate::usage::CardOp;

fn card(id: &str) -> Card {
    parse_card(id, &format!("---\ntitle: {id}\n---\nbody of {id}\n"))
}

fn library(ids: &[&str]) -> BTreeMap<String, Card> {
    ids.iter().map(|id| ((*id).to_owned(), card(id))).collect()
}

fn node(at: Timestamp, body: NodeBody) -> Node {
    Node {
        id: NodeId::new(),
        parent: None,
        session: Default::default(),
        at,
        dur_ms: None,
        body,
    }
}

fn note(content: Content) -> Node {
    node(
        0,
        NodeBody::Note {
            source: "memory".into(),
            content,
        },
    )
}

fn ok_result(call_id: &str) -> Node {
    node(
        0,
        NodeBody::ToolResult {
            call_id: call_id.into(),
            content: rumu_core::Content::text("ok"),
            is_error: false,
        },
    )
}

fn use_note(id: &str, op: CardOp, call_id: &str, at: Timestamp) -> Node {
    let mut n = node(
        at,
        NodeBody::Note {
            source: "workspace".into(),
            content: usage::card_use(id, "rev00000", "view0000", op, Some(call_id)),
        },
    );
    n.at = at;
    n
}

fn input<'a>(
    cards: &'a BTreeMap<String, Card>,
    scores: &'a BTreeMap<String, f64>,
    path_nodes: &'a [Node],
    unfolded: &'a [CatalogScope],
    default_workspace: Option<&'a str>,
    sessions: &'a [SessionTouch],
) -> CatalogInput<'a> {
    CatalogInput {
        cards,
        scores,
        path_nodes,
        unfolded,
        default_workspace,
        sessions,
        now_ms: 1_000_000,
    }
}

// ---------- ① small library ----------

#[test]
fn small_library_expands_everything() {
    let cards = library(&["infra", "preferences", "dev/rumu/kernel"]);
    let scores = BTreeMap::new();
    let plan = plan_root_catalog(&cards, &scores, ROOT_CATALOG_BUDGET);
    assert_eq!(plan.expanded.len(), 3);
    assert!(plan.folded.is_empty());
    let note = root_catalog_note(&cards, &plan);
    assert_eq!(note.scope, CatalogScope::Root);
    assert_eq!(note.entries.len(), 3);
    let text = note.markdown();
    assert!(text.starts_with("**Card catalog**"), "{text}");
    assert!(
        text.contains("[dev/rumu/kernel](@rumu:/card/dev/rumu/kernel)"),
        "{text}"
    );
    assert!(!text.contains("Folded directories"), "{text}");

    // The root note is injected once, then never again.
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &[], None, &[]));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].catalog.scope, CatalogScope::Root);
    let seen = [CatalogScope::Root];
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &seen, None, &[]));
    assert!(out.is_empty(), "root injected once: {out:?}");
}

// ---------- ② budget: top-level protection + Frecency + folding ----------

#[test]
fn budget_protects_top_level_and_prefers_frecency() {
    let mut ids: Vec<String> = vec!["top".into()];
    for i in 0..44 {
        ids.push(format!("dev/rumu/c{i:02}"));
    }
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let cards = library(&ids);
    let mut scores = BTreeMap::new();
    scores.insert("dev/rumu/c00".to_owned(), 100.0); // hot deep card
    for id in &ids {
        scores.entry((*id).to_owned()).or_insert(0.0);
    }

    let plan = plan_root_catalog(&cards, &scores, ROOT_CATALOG_BUDGET);
    assert!(plan.expanded.contains("top"), "top-level is protected");
    assert!(
        plan.expanded.contains("dev/rumu/c00"),
        "high Frecency deep card wins the remaining budget: {:?}",
        plan.expanded
    );
    assert_eq!(plan.expanded.len(), ROOT_CATALOG_BUDGET);
    assert_eq!(plan.folded.len(), 1);
    let folded = &plan.folded[0];
    assert_eq!(folded.prefix, "dev/rumu");
    assert_eq!(folded.cards.len(), 5); // 45 cards - 40 expanded

    let note = root_catalog_note(&cards, &plan);
    let text = note.markdown();
    assert!(text.contains("[top](@rumu:/card/top)"), "{text}");
    assert!(text.contains("Folded directories"), "{text}");
    assert!(text.contains("`dev/rumu/` (5 more cards: "), "{text}");
}

// ---------- ③ Frecency: use weights, imports, decay ----------

#[test]
fn frecency_weights_reads_writes_imports_and_decay() {
    let now = 10 * USE_HALF_LIFE_MS as Timestamp;
    let nodes = vec![
        ok_result("c1"),
        ok_result("c2"),
        use_note("w", CardOp::Write, "c1", now),
        use_note("r", CardOp::Read, "c2", now),
        // No successful result for c3: stale record, ignored.
        use_note("stale", CardOp::Read, "c3", now),
        // Auto-import notes never count.
        note(Content::note_with(
            "# imported\nbody",
            &crate::usage::CardImport {
                id: "imported".into(),
                rev: "a1b2c3d4".into(),
            },
        )),
        note(Content::marker(&WorkspaceUse {
            name: "proj".into(),
        })),
    ];
    let touch = extract_session_touch(&nodes);
    assert!(touch.workspaces.contains("proj"));
    assert_eq!(touch.card_uses["w"].1, 2.0);
    assert_eq!(touch.card_uses["r"].1, 1.0);
    assert!(!touch.card_uses.contains_key("stale"));
    assert!(!touch.card_uses.contains_key("imported"));

    let sessions = [touch];
    // Freshness: FRESH_WEIGHT at zero age, halved after one half-life.
    assert_eq!(score_card("x", Some(now), &[], now), FRESH_WEIGHT);
    assert_eq!(
        score_card("x", Some(now - FRESH_HALF_LIFE_MS as Timestamp), &[], now),
        FRESH_WEIGHT / 2.0
    );
    // Use decays with the 30-day half-life; writes weigh twice reads.
    assert_eq!(score_card("w", None, &sessions, now), 2.0);
    assert_eq!(score_card("r", None, &sessions, now), 1.0);
    let later = now + USE_HALF_LIFE_MS as Timestamp;
    assert_eq!(score_card("w", None, &sessions, later), 1.0);
    // A future mtime never scores above fresh (delta clamps to 0).
    assert_eq!(score_card("x", Some(now + 999), &[], now), FRESH_WEIGHT);
    // Workspace-scores only count sessions that touched that workspace.
    assert_eq!(score_card_for_workspace("w", "proj", &sessions, now), 2.0);
    assert_eq!(score_card_for_workspace("w", "else", &sessions, now), 0.0);
}

// ---------- ④ progressive directory unfolding ----------

#[test]
fn touching_a_folded_dir_unfolds_it_once() {
    let mut ids: Vec<String> = (0..41).map(|i| format!("dev/rumu/c{i:02}")).collect();
    ids.sort();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let cards = library(&ids);
    let scores: BTreeMap<String, f64> = cards.keys().map(|k| (k.clone(), 0.0)).collect();

    // The 41st card (folded) is read on this path; the root is already shown.
    let path_nodes = vec![
        ok_result("c1"),
        use_note("dev/rumu/c40", CardOp::Read, "c1", 1),
    ];
    let seen = [CatalogScope::Root];
    let out = plan_unfold_notes(&input(&cards, &scores, &path_nodes, &seen, None, &[]));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(
        out[0].catalog.scope,
        CatalogScope::Directory {
            prefix: "dev/rumu".into()
        }
    );
    let text = out[0].catalog.markdown();
    assert!(
        text.starts_with("**Cards under `@rumu:/card/dev/rumu/`:**"),
        "{text}"
    );
    assert!(text.contains("@rumu:/card/dev/rumu/c40"), "{text}");

    // The directory is already unfolded on the path: no repeat.
    let seen = [
        CatalogScope::Root,
        CatalogScope::Directory {
            prefix: "dev/rumu".into(),
        },
    ];
    let out = plan_unfold_notes(&input(&cards, &scores, &path_nodes, &seen, None, &[]));
    assert!(out.is_empty(), "{out:?}");

    // Not touching any folded dir: no dir note.
    let path_nodes = [ok_result("c1"), use_note("solo", CardOp::Read, "c1", 1)];
    let out = plan_unfold_notes(&input(&cards, &scores, &path_nodes, &seen, None, &[]));
    assert!(
        out.is_empty(),
        "unrelated card touch is not an unfold: {out:?}"
    );
}

// ---------- ⑤ progressive workspace unfolding ----------

#[test]
fn workspace_switch_pulls_in_unexpanded_workspace_cards() {
    // Two `proj` cards stay folded (cold); 43 hot cards fill the budget.
    let mut ids: Vec<String> = vec!["dev/proj/b".into(), "proj/a".into()];
    ids.extend((0..43).map(|i| format!("zzz/c{i:02}")));
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    let cards = library(&ids);
    let mut scores: BTreeMap<String, f64> = cards.keys().map(|k| (k.clone(), 1.0)).collect();
    scores.insert("dev/proj/b".to_owned(), 0.0);
    scores.insert("proj/a".to_owned(), 0.0);

    let seen = [CatalogScope::Root];
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &seen, Some("proj"), &[]));
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(
        out[0].catalog.scope,
        CatalogScope::Workspace {
            name: "proj".into()
        }
    );
    let text = out[0].catalog.markdown();
    assert!(
        text.starts_with("**Cards related to workspace `proj`:**"),
        "{text}"
    );
    assert!(text.contains("@rumu:/card/dev/proj/b"), "{text}");
    assert!(text.contains("@rumu:/card/proj/a"), "{text}");
    assert!(!text.contains("zzz/c"), "only matching cards: {text}");

    // Already unfolded: no repeat.
    let seen = [
        CatalogScope::Root,
        CatalogScope::Workspace {
            name: "proj".into(),
        },
    ];
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &seen, Some("proj"), &[]));
    assert!(out.is_empty(), "{out:?}");

    // `@rumu` is the built-in workspace: no ws note.
    let seen = [CatalogScope::Root];
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &seen, Some("@rumu"), &[]));
    assert!(out.is_empty(), "{out:?}");

    // A workspace with no unexpanded matches gets no empty note.
    let out = plan_unfold_notes(&input(&cards, &scores, &[], &seen, Some("ghost"), &[]));
    assert!(out.is_empty(), "{out:?}");
}
