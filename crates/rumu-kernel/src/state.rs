//! `state = f(path)`: deriving [`PathState`] by folding the root→head path,
//! then merging in the workspace registry.

use rumu_core::{
    CardMount, CardUnmount, ModelUse, Node, NodeBody, PROJECT_MOUNT, PathState, RUMU_WORKSPACE,
    SessionRoot, WorkspaceSet, WorkspaceUse, Workspaces,
};

/// Fold nodes (root → head, in order) into the derived path state.
///
/// This is the pure node-derived half: it yields the mount table (and with
/// it the project), the raw last `workspace.use` marker and a sub-agent's
/// restriction. `workspaces` and `mounted` stay empty; the kernel fills them
/// from the registry with [`merge_workspaces`].
pub fn derive_state(nodes: &[Node]) -> PathState {
    let mut st = PathState::default();
    for n in nodes {
        fold_node(&mut st, &n.body);
    }
    st.project = st
        .mounts
        .iter()
        .find(|m| m.at == PROJECT_MOUNT)
        .map(|m| m.deck.clone());
    st
}

/// Merge the node-derived state with the registry: every registered
/// workspace is addressable (or those a restriction names), the project's
/// list among them is mounted, and the default resolves marker → the
/// project's first → `@rumu`.
pub fn merge_workspaces(mut st: PathState, registry: &Workspaces) -> PathState {
    st.workspaces = registry
        .list()
        .into_iter()
        .filter(|w| {
            st.restrict
                .as_ref()
                .is_none_or(|names| names.contains(&w.name))
        })
        .collect();
    st.mounted = st
        .project
        .as_deref()
        .map(|p| registry.project(p))
        .unwrap_or_default()
        .into_iter()
        .filter(|n| st.workspaces.iter().any(|w| &w.name == n))
        .collect();
    let available =
        |name: &str| name == RUMU_WORKSPACE || st.workspaces.iter().any(|w| w.name == name);
    st.default_workspace = st
        .default_workspace
        .filter(|n| available(n))
        .or_else(|| st.mounted.first().cloned())
        .or_else(|| Some(RUMU_WORKSPACE.to_owned()));
    st
}

/// The project a mount-table marker moves its branch to: `Some(Some(deck))`
/// for a project mount, `Some(None)` for leaving the project, `None` when
/// the body is no such marker.
pub fn project_change(body: &NodeBody) -> Option<Option<String>> {
    let NodeBody::Note { content, .. } = body else {
        return None;
    };
    if let Some(mount) = content.decode::<CardMount>() {
        return (mount.at == PROJECT_MOUNT).then_some(Some(mount.deck));
    }
    if let Some(unmount) = content.decode::<CardUnmount>() {
        return (unmount.at == PROJECT_MOUNT).then_some(None);
    }
    None
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
        st.restrict = Some(set.names);
    } else if let Some(root) = content.decode::<SessionRoot>() {
        st.assistant = root.assistant.clone();
        st.asked_by = root.asked_by;
        // The session's own deck is always at `/cards/self`.
        st.mounts = vec![CardMount {
            deck: root.assistant,
            at: "/cards/self".into(),
        }];
        st.mounts
            .extend(root.mounts.into_iter().filter(|m| m.at != "/cards/self"));
    } else if let Some(mount) = content.decode::<CardMount>() {
        st.mounts.retain(|m| m.at != mount.at);
        st.mounts.push(mount);
    } else if let Some(unmount) = content.decode::<CardUnmount>() {
        st.mounts.retain(|m| m.at != unmount.at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rumu_core::{NodeId, Workspace};

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

    fn marker<T: rumu_core::Structured>(t: &T) -> Node {
        node(NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::marker(t),
        })
    }

    fn root(mounts: Vec<CardMount>) -> Node {
        marker(&SessionRoot {
            assistant: "x".into(),
            asked_by: None,
            mounts,
        })
    }

    fn project(deck: &str) -> CardMount {
        CardMount {
            deck: deck.into(),
            at: PROJECT_MOUNT.into(),
        }
    }

    fn names(st: &PathState) -> Vec<&str> {
        st.workspaces.iter().map(|w| w.name.as_str()).collect()
    }

    fn registry() -> Workspaces {
        let registry = Workspaces::new(vec![ws("a", "/a"), ws("b", "/b"), ws("c", "/c")]);
        registry
            .update_both(|_, projects| {
                projects.insert("p".into(), vec!["b".into(), "ghost".into(), "a".into()]);
                Ok::<_, ()>(())
            })
            .unwrap();
        registry
    }

    #[test]
    fn every_workspace_is_addressable_and_the_project_mounts_its_own() {
        let registry = registry();

        // No project: every workspace is addressable, none is mounted, and
        // `@rumu` is the default.
        let merged = merge_workspaces(derive_state(&[root(vec![])]), &registry);
        assert_eq!(names(&merged), ["a", "b", "c"]);
        assert!(merged.mounted.is_empty() && merged.project.is_none());
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));

        // A project from the root mounts its registered workspaces in its
        // order; the first is the default.
        let merged = merge_workspaces(derive_state(&[root(vec![project("p")])]), &registry);
        assert_eq!(merged.project.as_deref(), Some("p"));
        assert_eq!(merged.mounted, ["b", "a"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("b"));

        // A marker default wins when it is addressable, mounted or not.
        let merged = merge_workspaces(
            derive_state(&[root(vec![project("p")]), use_("c")]),
            &registry,
        );
        assert_eq!(merged.default_workspace.as_deref(), Some("c"));

        // Moving in and out of a project follows the mount markers.
        let moved = derive_state(&[root(vec![]), marker(&project("p"))]);
        assert_eq!(moved.project.as_deref(), Some("p"));
        let left = derive_state(&[
            root(vec![project("p")]),
            marker(&CardUnmount {
                at: PROJECT_MOUNT.into(),
            }),
        ]);
        assert!(left.project.is_none());
    }

    #[test]
    fn a_restriction_narrows_what_the_branch_addresses() {
        let registry = registry();
        let merged = merge_workspaces(
            derive_state(&[root(vec![project("p")]), use_("b"), set(&["a", "ghost"])]),
            &registry,
        );
        assert_eq!(names(&merged), ["a"]);
        assert_eq!(merged.mounted, ["a"]);
        // `b` is out of reach now, so the default falls back to the first
        // mounted one.
        assert_eq!(merged.default_workspace.as_deref(), Some("a"));
        // `@rumu` stays a valid default.
        let merged = merge_workspaces(derive_state(&[set(&[]), use_("@rumu")]), &registry);
        assert!(merged.workspaces.is_empty());
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));
    }

    #[test]
    fn project_changes_are_the_project_mount_markers() {
        assert_eq!(
            project_change(&marker(&project("p")).body),
            Some(Some("p".into()))
        );
        assert_eq!(
            project_change(
                &marker(&CardUnmount {
                    at: PROJECT_MOUNT.into()
                })
                .body
            ),
            Some(None)
        );
        let other = CardMount {
            deck: "d".into(),
            at: "/cards/user".into(),
        };
        assert_eq!(project_change(&marker(&other).body), None);
        assert_eq!(project_change(&use_("a").body), None);
    }
}
