//! `state = f(path)`: deriving [`PathState`] by folding the root→head path,
//! then merging in the workspace registry.

use rumu_core::{
    ModelUse, Node, NodeBody, PathState, RUMU_WORKSPACE, SessionRoot, Workspace, WorkspaceSet,
    WorkspaceUse,
};

/// Fold nodes (root → head, in order) into the derived path state.
///
/// This is the pure node-derived half: it yields the attached workspace
/// names and the raw last `workspace.use` marker. `workspaces` stays empty;
/// the kernel fills it from the registry with [`merge_workspaces`].
pub fn derive_state(nodes: &[Node]) -> PathState {
    let mut st = PathState::default();
    for n in nodes {
        fold_node(&mut st, &n.body);
    }
    st
}

/// Merge the node-derived state with the registry: the attached names that
/// are registered become the available workspaces (a removed workspace just
/// drops out), and the default resolves marker → first attached → `@rumu`.
pub fn merge_workspaces(mut st: PathState, registry: &[Workspace]) -> PathState {
    st.workspaces = st
        .attached
        .iter()
        .filter_map(|n| registry.iter().find(|w| &w.name == n).cloned())
        .collect();
    let available =
        |name: &str| name == RUMU_WORKSPACE || st.workspaces.iter().any(|w| w.name == name);
    st.default_workspace = st
        .default_workspace
        .filter(|n| available(n))
        .or_else(|| st.workspaces.first().map(|w| w.name.clone()))
        .or_else(|| Some(RUMU_WORKSPACE.to_owned()));
    st
}

fn fold_node(st: &mut PathState, body: &NodeBody) {
    let NodeBody::Note { content, .. } = body else {
        return;
    };
    if let Some(model) = content.decode::<ModelUse>() {
        st.model = Some(model);
    } else if let Some(use_) = content.decode::<WorkspaceUse>() {
        st.default_workspace = Some(use_.name);
    } else if let Some(set) = content.decode::<WorkspaceSet>() {
        st.attached = set.names;
    } else if let Some(root) = content.decode::<SessionRoot>() {
        st.assistant = root.assistant.clone();
        st.asked_by = root.asked_by;
        st.attached = root.workspaces;
        // The session's own deck is always at `/cards/self`.
        st.mounts = vec![rumu_core::CardMount {
            deck: root.assistant,
            at: "/cards/self".into(),
        }];
        st.mounts
            .extend(root.mounts.into_iter().filter(|m| m.at != "/cards/self"));
    } else if let Some(mount) = content.decode::<rumu_core::CardMount>() {
        st.mounts.retain(|m| m.at != mount.at);
        st.mounts.push(mount);
    } else if let Some(unmount) = content.decode::<rumu_core::CardUnmount>() {
        st.mounts.retain(|m| m.at != unmount.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::NodeId;

    fn node(body: NodeBody) -> Node {
        Node {
            id: NodeId::new(),
            parent: None,
            session: rumu_core::SessionId::new(),
            at: 0,
            dur_ms: None,
            body,
        }
    }

    fn ws(name: &str, root: &str) -> Workspace {
        Workspace {
            name: name.into(),
            ssh: None,
            root: root.into(),
            read_only: false,
        }
    }

    #[test]
    fn notes_and_workspace_use() {
        // Unknown marker tags are ignored.
        let nodes = vec![
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(rumu_core::Tagged {
                        tag: "tag".into(),
                        v: 1,
                        data: serde_json::json!({"name": "infra"}),
                    }),
                },
            }),
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(rumu_core::Tagged {
                        tag: "checkpoint".into(),
                        v: 1,
                        data: serde_json::json!({"label": "cp"}),
                    }),
                },
            }),
            node(NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content::text("t"),
            }),
            // The latest workspace_use marker is the default.
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::WorkspaceUse { name: "a".into() }),
            }),
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::WorkspaceUse { name: "b".into() }),
            }),
        ];

        let st = derive_state(&nodes);
        assert!(st.workspaces.is_empty());
        assert_eq!(st.default_workspace.as_deref(), Some("b"));
    }

    fn set(names: &[&str]) -> Node {
        node(NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::marker(&WorkspaceSet {
                names: names.iter().map(|n| (*n).to_owned()).collect(),
            }),
        })
    }

    fn use_(name: &str) -> Node {
        node(NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::marker(&WorkspaceUse { name: name.into() }),
        })
    }

    fn names(st: &PathState) -> Vec<&str> {
        st.workspaces.iter().map(|w| w.name.as_str()).collect()
    }

    #[test]
    fn attached_workspaces_resolve_against_the_registry() {
        let registry = [ws("a", "/a"), ws("b", "/b"), ws("c", "/c")];

        // Nothing attached: only `@rumu`, which is the default.
        let merged = merge_workspaces(derive_state(&[]), &registry);
        assert!(merged.workspaces.is_empty());
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));

        // The root attaches its list in order; the first is the default.
        let root = node(NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::marker(&SessionRoot {
                assistant: "x".into(),
                asked_by: None,
                mounts: Vec::new(),
                workspaces: vec!["b".into(), "a".into()],
            }),
        });
        let merged = merge_workspaces(derive_state(std::slice::from_ref(&root)), &registry);
        assert_eq!(names(&merged), ["b", "a"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("b"));

        // A later set replaces the list; a marker default inside it wins.
        let merged = merge_workspaces(
            derive_state(&[root.clone(), use_("c"), set(&["a", "c"])]),
            &registry,
        );
        assert_eq!(names(&merged), ["a", "c"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("c"));

        // A default that is no longer attached falls back to the first.
        let merged = merge_workspaces(derive_state(&[use_("b"), set(&["c"])]), &registry);
        assert_eq!(merged.default_workspace.as_deref(), Some("c"));

        // An unregistered name drops out; `@rumu` stays a valid default.
        let merged = merge_workspaces(
            derive_state(&[set(&["ghost", "a"]), use_("@rumu")]),
            &registry,
        );
        assert_eq!(names(&merged), ["a"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));
    }
}
