//! `state = f(path)`: deriving [`PathState`] by folding the root→head path,
//! then merging in the workspace registry.

use rumu_core::{
    CardMount, CardUnmount, ModelReset, ModelUse, Node, NodeBody, PathState, RUMU_WORKSPACE,
    SCRATCH_WORKSPACE, SessionId, SessionRoot, WorkspaceSet, WorkspaceUse, Workspaces,
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
    st
}

/// Merge the node-derived state with the registry: every registered
/// workspace is addressable (or those a restriction names), then the
/// session's `@scratch`; the project's list among them is mounted, and the
/// default resolves marker → the project's first → `@scratch`.
pub fn merge_workspaces(
    mut st: PathState,
    registry: &Workspaces,
    session: &SessionId,
) -> PathState {
    st.workspaces = registry
        .list()
        .into_iter()
        .filter(|w| {
            st.restrict
                .as_ref()
                .is_none_or(|names| names.contains(&w.name))
        })
        .chain([registry.scratch(session)])
        .collect();
    st.mounted = st
        .project()
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
        .or_else(|| Some(SCRATCH_WORKSPACE.to_owned()));
    st
}

/// Is `body` a mount or unmount marker (which may change its branch's
/// project)?
pub fn mounts_changed(body: &NodeBody) -> bool {
    let NodeBody::Note { content, .. } = body else {
        return false;
    };
    content.decode::<CardMount>().is_some() || content.decode::<CardUnmount>().is_some()
}

/// Mount `mount` (replacing what is at its point). A path is in one project
/// at most: a project mount while it has one is a plain mount (only one
/// short-lived version wrote such paths, and the session listed the first).
fn mount(st: &mut PathState, mut mount: CardMount) {
    st.mounts.retain(|m| m.at != mount.at);
    mount.project &= st.project_mount().is_none();
    st.mounts.push(mount);
}

fn fold_node(st: &mut PathState, body: &NodeBody) {
    let NodeBody::Note { content, .. } = body else {
        return;
    };
    let project = st.project_mount().cloned();
    fold_marker(st, content);
    // Entering, switching or leaving a project drops the `workspace.use`
    // before it: the new project's own default applies.
    if st.project_mount() != project.as_ref() {
        st.default_workspace = None;
    }
}

fn fold_marker(st: &mut PathState, content: &rumu_core::Content) {
    if let Some(model) = content.decode::<ModelUse>() {
        st.model = Some(model);
    } else if content.decode::<ModelReset>().is_some() {
        st.model = None;
    } else if let Some(use_) = content.decode::<WorkspaceUse>() {
        st.default_workspace = Some(use_.name);
    } else if let Some(set) = content.decode::<WorkspaceSet>() {
        st.restrict = Some(set.names);
    } else if let Some(root) = content.decode::<SessionRoot>() {
        st.assistant = root.assistant.clone();
        // The session's own deck is always at `/cards/self`.
        st.mounts = vec![CardMount::new(root.assistant, "/cards/self", false)];
        for m in root.mounts.into_iter().filter(|m| m.at != "/cards/self") {
            mount(st, m);
        }
    } else if let Some(m) = content.decode::<CardMount>() {
        mount(st, m);
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
            // The latest `workspace.use` marker is the default.
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
            mounts,
        })
    }

    fn project(deck: &str) -> CardMount {
        CardMount::new(deck, format!("/cards/{deck}"), true)
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
        // `@scratch` is the default.
        let merged = merge_workspaces(derive_state(&[root(vec![])]), &registry, &SessionId::new());
        assert_eq!(names(&merged), ["a", "b", "c", "@scratch"]);
        assert!(merged.mounted.is_empty() && merged.project().is_none());
        assert_eq!(merged.default_workspace.as_deref(), Some("@scratch"));

        // A project from the root mounts its registered workspaces in its
        // order; the first is the default.
        let merged = merge_workspaces(
            derive_state(&[root(vec![project("p")])]),
            &registry,
            &SessionId::new(),
        );
        assert_eq!(merged.project(), Some("p"));
        assert_eq!(merged.mounted, ["b", "a"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("b"));

        // A marker default wins when it is addressable, mounted or not.
        let merged = merge_workspaces(
            derive_state(&[root(vec![project("p")]), use_("c")]),
            &registry,
            &SessionId::new(),
        );
        assert_eq!(merged.default_workspace.as_deref(), Some("c"));

        // Moving in and out of a project follows the mount markers.
        let moved = derive_state(&[root(vec![]), marker(&project("p"))]);
        assert_eq!(moved.project(), Some("p"));
        let left = derive_state(&[
            root(vec![project("p")]),
            marker(&CardUnmount {
                at: "/cards/p".into(),
            }),
        ]);
        assert!(left.project().is_none());

        // Entering or leaving a project drops the `workspace.use` before it;
        // one after it stays.
        let st =
            |nodes: &[Node]| merge_workspaces(derive_state(nodes), &registry, &SessionId::new());
        let entered = st(&[root(vec![]), use_("c"), marker(&project("p"))]);
        assert_eq!(entered.default_workspace.as_deref(), Some("b"));
        let left = st(&[
            root(vec![project("p")]),
            use_("c"),
            marker(&CardUnmount {
                at: "/cards/p".into(),
            }),
        ]);
        assert_eq!(left.default_workspace.as_deref(), Some(SCRATCH_WORKSPACE));
        let borrowed = CardMount::new("q", "/cards/q", false);
        let kept = st(&[root(vec![project("p")]), use_("c"), marker(&borrowed)]);
        assert_eq!(kept.default_workspace.as_deref(), Some("c"));
    }

    #[test]
    fn a_path_is_in_one_project_at_most() {
        // An old journal's project, at the point it then had.
        let old: CardMount =
            serde_json::from_value(serde_json::json!({"deck": "p", "at": "/cards/project"}))
                .unwrap();
        assert_eq!(derive_state(&[root(vec![old])]).project(), Some("p"));
        // Another project deck mounted plainly leaves the project as it is.
        let borrowed = CardMount::new("q", "/cards/q", false);
        let st = derive_state(&[root(vec![project("p")]), marker(&borrowed)]);
        assert_eq!(st.project(), Some("p"));
        assert_eq!(st.mounts.len(), 3);
        // A path stored with two projects keeps the first; the later one
        // stays mounted as any deck.
        let st = derive_state(&[
            root(vec![project("p"), project("q")]),
            marker(&project("r")),
        ]);
        assert_eq!(st.project(), Some("p"));
        assert_eq!(st.mounts.iter().filter(|m| m.project).count(), 1);
        assert_eq!(st.mounts.len(), 4);
        // Remounting the project's point replaces it.
        let st = derive_state(&[
            root(vec![project("p")]),
            marker(&CardMount::new("q", "/cards/p", true)),
        ]);
        assert_eq!(st.project(), Some("q"));
    }

    #[test]
    fn a_restriction_narrows_what_the_branch_addresses() {
        let registry = registry();
        let merged = merge_workspaces(
            derive_state(&[root(vec![project("p")]), use_("b"), set(&["a", "ghost"])]),
            &registry,
            &SessionId::new(),
        );
        assert_eq!(names(&merged), ["a", "@scratch"]);
        assert_eq!(merged.mounted, ["a"]);
        // `b` is out of reach now, so the default falls back to the first
        // mounted one.
        assert_eq!(merged.default_workspace.as_deref(), Some("a"));
        // `@rumu` stays a valid default; `@scratch` is still addressable
        // even though the restriction names no workspace.
        let merged = merge_workspaces(
            derive_state(&[set(&[]), use_("@rumu")]),
            &registry,
            &SessionId::new(),
        );
        assert_eq!(names(&merged), ["@scratch"]);
        assert_eq!(merged.default_workspace.as_deref(), Some("@rumu"));
    }

    #[test]
    fn scratch_is_the_sessions_directory_and_survives_restrictions() {
        let session = SessionId::new();
        let registry = registry().with_scratch(std::path::PathBuf::from("/scratch"));

        // A restriction makes it the only registered workspace plus
        // `@scratch`, whose root is the session's own directory.
        let merged = merge_workspaces(derive_state(&[set(&["a"])]), &registry, &session);
        assert_eq!(names(&merged), ["a", "@scratch"]);
        let scratch = merged.workspaces.last().expect("`@scratch` is last");
        assert_eq!(scratch.name, SCRATCH_WORKSPACE);
        assert_eq!(scratch.root, registry.scratch(&session).root);
        assert!(scratch.root.ends_with(session.to_string().as_str()));
        // No marker and no project: it is the default.
        assert_eq!(merged.default_workspace.as_deref(), Some(SCRATCH_WORKSPACE));
    }

    #[test]
    fn mount_markers_change_the_mounts() {
        assert!(mounts_changed(&marker(&project("p")).body));
        let gone = CardUnmount {
            at: "/cards/user".into(),
        };
        assert!(mounts_changed(&marker(&gone).body));
        assert!(!mounts_changed(&use_("a").body));
    }

    #[test]
    fn a_reset_returns_the_path_to_its_assistants_model() {
        let start = root(Vec::new());
        assert_eq!(derive_state(std::slice::from_ref(&start)).model, None);
        let picked = ModelUse {
            model: "a/m".into(),
            effort: Some(rumu_core::Effort::Low),
        };
        let path = [start, marker(&picked)];
        assert_eq!(derive_state(&path).model, Some(picked));
        let path = [path[0].clone(), path[1].clone(), marker(&ModelReset {})];
        assert_eq!(derive_state(&path).model, None);
    }
}
