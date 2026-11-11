//! A per-path index of structured note payloads.
//!
//! Every [`crate::NodeBody::Note`] with `content.data` is indexed, markers
//! (empty text) included; plain notes without data are not. The index is built
//! lazily once per [`crate::Cx`] from the same view as
//! [`crate::Cx::path`] (what the model sees, plus effects queued for the next
//! safe point), so a plugin can query what earlier calls in its batch already
//! recorded without the kernel deduplicating anything on its behalf.

use serde::{Deserialize, Serialize};

use crate::{Node, NodeBody, NodeId, Structured, Tagged};

/// One raw indexed payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedNote {
    pub node: NodeId,
    pub source: String,
    pub data: Tagged,
}

/// One decoded payload of type `T`, in path order.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry<T> {
    pub node: NodeId,
    pub source: String,
    pub value: T,
}

/// The structured view of a branch path. Build it through [`crate::Cx::index`].
#[derive(Debug, Clone, Default)]
pub struct Index {
    entries: Vec<IndexedNote>,
}

impl Index {
    /// Index every structured note on `nodes` (already root→head order).
    pub fn build(nodes: &[Node]) -> Self {
        let entries = nodes
            .iter()
            .filter_map(|n| {
                let NodeBody::Note { source, content } = &n.body else {
                    return None;
                };
                Some(IndexedNote {
                    node: n.id.clone(),
                    source: source.clone(),
                    data: content.data.clone()?,
                })
            })
            .collect();
        Self { entries }
    }

    /// Every decoded `T` payload, in path order.
    pub fn all<T: Structured>(&self) -> Vec<Entry<T>> {
        self.entries
            .iter()
            .filter_map(|e| {
                e.data.decode::<T>().map(|value| Entry {
                    node: e.node.clone(),
                    source: e.source.clone(),
                    value,
                })
            })
            .collect()
    }

    /// The latest decoded `T` payload, if any.
    pub fn latest<T: Structured>(&self) -> Option<Entry<T>> {
        self.all::<T>().pop()
    }

    /// Raw payloads carrying `tag` (any version), for untyped callers.
    pub fn by_tag<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a IndexedNote> + 'a {
        self.entries.iter().filter(move |e| e.data.tag == tag)
    }

    /// True when no structured note is on the path.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Content, SessionId};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Thing {
        n: u32,
    }

    impl Structured for Thing {
        const TAG: &'static str = "test.thing";
        const V: u32 = 1;
    }

    fn note(id: &str, source: &str, content: Content) -> Node {
        Node {
            id: NodeId::from(id),
            parent: None,
            session: SessionId::new(),
            at: 0,
            dur_ms: None,
            body: NodeBody::Note {
                source: source.into(),
                content,
            },
        }
    }

    #[test]
    fn indexes_structured_notes_in_path_order() {
        let nodes = vec![
            note("a", "one", Content::marker(&Thing { n: 1 })),
            note("b", "two", Content::text("plain")),
            note("c", "three", Content::note_with("hi", &Thing { n: 2 })),
            note("d", "four", Content::marker(&Thing { n: 3 })),
        ];
        let index = Index::build(&nodes);
        assert_eq!(
            index
                .by_tag(Thing::TAG)
                .map(|e| e.node.0.as_str())
                .collect::<Vec<_>>(),
            ["a", "c", "d"]
        );
        let all = index.all::<Thing>();
        assert_eq!(
            all.iter()
                .map(|e| (e.node.0.as_str(), e.source.as_str(), e.value.n))
                .collect::<Vec<_>>(),
            [("a", "one", 1), ("c", "three", 2), ("d", "four", 3)]
        );
        assert_eq!(index.latest::<Thing>().unwrap().value.n, 3);
        assert!(Index::build(&[]).is_empty());
    }
}
