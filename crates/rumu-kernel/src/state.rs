//! `state = f(path)`: deriving [`PathState`] by folding the root→head path,
//! then merging in the workspace preset catalog.

use rumu_core::{
    Node, NodeBody, PathState, RUMU_WORKSPACE, WorkspaceCatalog, WorkspaceScope, WorkspaceUse,
};

/// Fold nodes (root → head, in order) into the derived path state.
///
/// This is the pure node-derived half: it yields the raw last
/// `workspace.use` marker and the intersection of every `workspace.scope`
/// marker. `workspaces` stays empty; the kernel fills it from the preset
/// catalog with [`merge_workspaces`].
pub fn derive_state(nodes: &[Node]) -> PathState {
    let mut st = PathState::default();
    for n in nodes {
        fold_node(&mut st, &n.body);
    }
    st
}

/// Merge the node-derived state with the preset catalog: every preset (or,
/// under a `workspace.scope`, every allowed preset) becomes available, and
/// the default resolves marker → preset `default` → first available, kept
/// within the scope. `@rumu` is always a valid default.
pub fn merge_workspaces(mut st: PathState, catalog: &WorkspaceCatalog) -> PathState {
    st.workspaces = catalog.workspaces.clone();
    if let Some(scope) = &st.workspace_scope {
        st.workspaces.retain(|w| scope.iter().any(|n| n == &w.name));
    }
    let available =
        |name: &str| name == RUMU_WORKSPACE || st.workspaces.iter().any(|w| w.name == name);
    let scoped = st.workspace_scope.is_some();
    st.default_workspace = st
        .default_workspace
        .filter(|n| available(n))
        .or_else(|| catalog.default.clone().filter(|n| available(n)))
        .or_else(|| st.workspaces.first().map(|w| w.name.clone()))
        // `@rumu` is always allowed, so it is the fallback under any scope.
        .or_else(|| scoped.then(|| RUMU_WORKSPACE.to_owned()));
    st
}

fn fold_node(st: &mut PathState, body: &NodeBody) {
    let NodeBody::Note { content, .. } = body else {
        return;
    };
    if let Some(use_) = content.decode::<WorkspaceUse>() {
        st.default_workspace = Some(use_.name);
    } else if let Some(scope) = content.decode::<WorkspaceScope>() {
        // Nested scopes can only narrow: keep the intersection, in the
        // first scope's order.
        st.workspace_scope = Some(match st.workspace_scope.take() {
            None => scope.allow,
            Some(prev) => prev
                .into_iter()
                .filter(|n| scope.allow.iter().any(|a| a == n))
                .collect(),
        });
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

    fn catalog(workspaces: Vec<Workspace>, default: Option<&str>) -> WorkspaceCatalog {
        WorkspaceCatalog {
            workspaces,
            default: default.map(str::to_owned),
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

    #[test]
    fn merge_adds_presets_and_resolves_default() {
        // Marker default wins over the preset default; every preset is available.
        let st = PathState {
            default_workspace: Some("life".into()),
            ..Default::default()
        };
        let cat = catalog(
            vec![ws("proj", "/presets/proj"), ws("life", "/presets/life")],
            Some("proj"),
        );
        let merged = merge_workspaces(st, &cat);
        assert_eq!(
            merged
                .workspaces
                .iter()
                .map(|w| (w.name.as_str(), w.root.as_str()))
                .collect::<Vec<_>>(),
            [("proj", "/presets/proj"), ("life", "/presets/life")]
        );
        assert_eq!(merged.default_workspace.as_deref(), Some("life"));

        // No marker → preset default.
        let merged = merge_workspaces(PathState::default(), &cat);
        assert_eq!(merged.default_workspace.as_deref(), Some("proj"));

        // No marker, no preset default → first preset.
        let cat2 = catalog(
            vec![ws("proj", "/presets/proj"), ws("life", "/presets/life")],
            None,
        );
        let merged = merge_workspaces(PathState::default(), &cat2);
        assert_eq!(merged.default_workspace.as_deref(), Some("proj"));

        // No presets at all → no default.
        let merged = merge_workspaces(PathState::default(), &WorkspaceCatalog::default());
        assert_eq!(merged.default_workspace, None);

        // A marker naming an unknown workspace is ignored in the merge.
        let st = PathState {
            default_workspace: Some("ghost".into()),
            ..Default::default()
        };
        let merged = merge_workspaces(st, &cat);
        assert_eq!(merged.default_workspace.as_deref(), Some("proj"));

        // `@rumu` is a valid default too.
        let st = PathState {
            default_workspace: Some("@rumu".into()),
            ..Default::default()
        };
        let merged = merge_workspaces(st, &cat);
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));
    }

    fn scope(names: &[&str]) -> Node {
        node(NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::marker(&rumu_core::WorkspaceScope {
                allow: names.iter().map(|n| (*n).to_owned()).collect(),
            }),
        })
    }

    fn names(st: &PathState) -> Vec<&str> {
        st.workspaces.iter().map(|w| w.name.as_str()).collect()
    }

    #[test]
    fn scope_markers_filter_and_intersect() {
        let cat = catalog(vec![ws("a", "/a"), ws("b", "/b"), ws("c", "/c")], Some("a"));

        // One scope: only allowed presets remain and the default is kept.
        let st = derive_state(&[scope(&["a", "b", "@rumu"])]);
        let merged = merge_workspaces(st, &cat);
        assert_eq!(names(&merged), ["a", "b"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("a"));

        // Nested scopes intersect; an out-of-scope preset default falls back
        // to the first allowed workspace.
        let st = derive_state(&[scope(&["a", "b"]), scope(&["b", "c"])]);
        assert_eq!(
            st.workspace_scope.as_deref(),
            Some(["b".to_owned()].as_slice())
        );
        let merged = merge_workspaces(st, &cat);
        assert_eq!(names(&merged), ["b"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("b"));

        // A `workspace.use` default inside the scope wins.
        let st = derive_state(&[
            node(NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::WorkspaceUse { name: "b".into() }),
            }),
            scope(&["a", "b"]),
        ]);
        let merged = merge_workspaces(st, &cat);
        assert_eq!(merged.default_workspace.as_deref(), Some("b"));

        // `@rumu` is always allowed, and is a valid default when it is the
        // only thing in scope.
        let merged = merge_workspaces(derive_state(&[scope(&["@rumu"])]), &cat);
        assert!(merged.workspaces.is_empty());
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));

        // No scope: every preset stays available.
        let merged = merge_workspaces(derive_state(&[]), &cat);
        assert_eq!(names(&merged), ["a", "b", "c"]);
    }
}
