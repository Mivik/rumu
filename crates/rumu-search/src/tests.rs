use super::*;
use rumu_core::{Content, NodeId, Timestamp};

fn node(session: &str, id: &str, at: Timestamp, body: NodeBody) -> Node {
    Node {
        id: NodeId(id.into()),
        parent: None,
        session: SessionId(session.into()),
        at,
        dur_ms: None,
        body,
    }
}

fn user(session: &str, id: &str, at: Timestamp, text: &str) -> Node {
    let content = Content::text(text);
    node(session, id, at, NodeBody::User { content })
}

fn ids(found: &Found) -> Vec<&str> {
    found.hits.iter().map(|h| h.node.as_str()).collect()
}

fn sid(s: &str) -> SessionId {
    SessionId(s.into())
}

#[test]
fn chinese_is_segmented_and_latin_lowercased() {
    let index = SessionIndex::in_memory();
    index.upsert(&user("s", "zh", 1, "我们讨论了倒排索引的实现"), 10);
    index.upsert(&user("s", "en", 2, "Inverted Index in Rust"), 20);
    let all = [sid("s")];
    assert_eq!(
        ids(&index.search("倒排索引怎么做", &all, 10).unwrap()),
        ["zh"]
    );
    assert_eq!(ids(&index.search("INVERTED", &all, 10).unwrap()), ["en"]);
    let found = index.search("index 索引", &all, 10).unwrap();
    assert_eq!(found.total, 2);
    assert_eq!(found.hits[0].text, "Inverted Index in Rust");
    // Snippets point at the query as written.
    assert_eq!(query_terms("Index 索引"), ["index", "索引"]);
}

#[test]
fn sessions_filter_tool_results_count_half_and_ties_are_newest_first() {
    let index = SessionIndex::in_memory();
    let result = NodeBody::ToolResult {
        call_id: "c".into(),
        content: Content::text("deploy"),
        is_error: false,
        invoke: None,
    };
    index.upsert(&node("a", "tool", 3, result), 1);
    index.upsert(&user("a", "old", 1, "deploy"), 2);
    index.upsert(&user("a", "new", 2, "deploy"), 3);
    index.upsert(&user("b", "other", 4, "deploy"), 4);
    let found = index.search("deploy", &[sid("a")], 10).unwrap();
    assert_eq!(ids(&found), ["new", "old", "tool"]);
    assert_eq!(found.total, 3);
    assert_eq!(index.search("deploy", &[sid("a")], 1).unwrap().total, 3);
    assert!(index.search("deploy", &[], 10).unwrap().hits.is_empty());
    // A marker is indexed (its id counts) but matches nothing.
    let marker = NodeBody::Note {
        source: "system".into(),
        content: Content::marker(&rumu_core::SessionRoot::default()),
    };
    index.upsert(&node("a", "mark", 5, marker), 5);
    // The same node twice is one document.
    index.upsert(&user("a", "new", 2, "deploy"), 6);
    index.flush();
    let mut nodes = index.node_ids(&sid("a")).unwrap();
    nodes.sort();
    assert_eq!(nodes, ["mark", "new", "old", "tool"]);
    index.delete_session(&sid("a"));
    assert_eq!(
        ids(&index.search("deploy", &[sid("a"), sid("b")], 10).unwrap()),
        ["other"]
    );
}

#[test]
fn lengths_persist_and_a_stale_session_waits_for_its_reindex() {
    let dir = std::env::temp_dir().join(format!("rumu-search-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    {
        let (index, lens) = SessionIndex::open(&dir).unwrap();
        assert!(lens.is_empty());
        index.upsert(&user("a", "n1", 1, "hello"), 100);
        index.touch(&sid("a"), 150);
        index.stale(vec!["b".into()]);
        index.upsert(&user("b", "n2", 2, "hello"), 70);
        index.flush();
    }
    {
        let (index, lens) = SessionIndex::open(&dir).unwrap();
        // `b` was mid re-index: it has no length, so it is re-indexed again.
        assert_eq!(lens, BTreeMap::from([("a".into(), 150)]));
        assert_eq!(
            index
                .search("hello", &[sid("a"), sid("b")], 10)
                .unwrap()
                .total,
            2
        );
        index.stale(vec!["b".into()]);
        index.upsert(&user("b", "n3", 3, "later"), 90);
        index.reindexed(&sid("b"), 80);
        index.flush();
    }
    let (_, lens) = SessionIndex::open(&dir).unwrap();
    assert_eq!(lens.get("b"), Some(&90));
    // Another version starts over.
    {
        let (index, _) = SessionIndex::open(&dir).unwrap();
        let payload = serde_json::to_string(&Payload {
            version: VERSION + 1,
            sessions: BTreeMap::new(),
        })
        .unwrap();
        let mut writer = index.shared.writer.write().unwrap();
        let mut prepared = writer.prepare_commit().unwrap();
        prepared.set_payload(&payload);
        prepared.commit().unwrap();
    }
    let (index, lens) = SessionIndex::open(&dir).unwrap();
    assert!(lens.is_empty());
    assert_eq!(index.search("hello", &[sid("a")], 10).unwrap().total, 0);
    std::fs::remove_dir_all(&dir).ok();
}
