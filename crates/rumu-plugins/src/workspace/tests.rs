//! Tests for the workspace plugin: `<ws>:<path>` resolution, the file/bash
//! tools over local backends (absolute paths included), `workspace_use`, the
//! `@rumu` virtual workspace (`/cards` reads/writes with validation and
//! read-before-write, `/sessions` views, `/tasks` reads, unknown roots), AGENTS.md
//! injection and the config.yaml loader. The ssh backend's command construction
//! is tested in `backend.rs` (no real ssh).

use super::*;
use crate::testutil::FakeRuntime;
use rumu_cards::{card, usage};
use rumu_core::{
    Node, NodeBody, PathState, RUMU_WORKSPACE, SCRATCH_WORKSPACE, Structured, Tagged, ToolOutput,
    WorkspaceUse,
};
use serde_json::json;

fn ws(name: &str, root: &str) -> Workspace {
    Workspace {
        name: name.into(),
        ssh: None,
        root: root.into(),
        read_only: false,
    }
}

fn state_with(workspaces: Vec<Workspace>, default: Option<&str>) -> PathState {
    PathState {
        workspaces,
        default_workspace: default.map(str::to_owned),
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        mounts: vec![rumu_core::CardMount {
            deck: crate::testutil::MAIN_DECK.into(),
            at: "/cards/self".into(),
        }],
        ..Default::default()
    }
}

/// A plugin + host wired to a temp RUMU_HOME and a single local workspace.
/// The returned tempdir must stay alive for the plugin's lifetime.
fn setup(
    root: &Path,
    read_only: bool,
) -> (
    WorkspacePlugin,
    std::sync::Arc<FakeRuntime>,
    rumu_core::Cx,
    tempfile::TempDir,
) {
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    let plugin = deck_plugin(home.path());
    let mut wsb = ws("proj", &root.to_string_lossy());
    wsb.read_only = read_only;
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = state_with(vec![wsb], Some("proj"));
    let cx = host.cx();
    (plugin, host, cx, home)
}

/// Mirror the kernel's safe point for one tool output (see
/// `FakeRuntime::apply_output`).
fn flush_output(host: &std::sync::Arc<FakeRuntime>, call_id: &str, out: &ToolOutput) {
    host.apply_output(call_id, out);
}

/// A workspace plugin over the temp home's decks, with an empty workspace
/// registry (tests that need one use [`deck_plugin_with`]).
fn deck_plugin(home: &Path) -> WorkspacePlugin {
    deck_plugin_with(home, rumu_core::Workspaces::default())
}

/// A workspace plugin sharing `workspaces` with the test.
fn deck_plugin_with(home: &Path, workspaces: rumu_core::Workspaces) -> WorkspacePlugin {
    let decks = rumu_cards::Decks::load(home).unwrap();
    WorkspacePlugin::new(home, decks, workspaces)
}

/// One card read from a deck's `HEAD` tree.
fn deck_card_in(home: &Path, deck: &str, path: &str) -> rumu_cards::Card {
    let text = crate::testutil::card_text(home, deck, path).unwrap();
    rumu_cards::card::parse_card(path, &text)
}

/// One card read straight from the `main` deck on disk.
fn deck_card(home: &Path, path: &str) -> rumu_cards::Card {
    deck_card_in(home, "main", path)
}

// ---------- resolution ----------

#[tokio::test]
async fn address_resolution_errors() {
    let home = tempfile::tempdir().unwrap();
    let plugin = deck_plugin(home.path());
    let host = FakeRuntime::arc();

    // No default workspace: the error points at `workspace_use()`.
    *host.state.lock().unwrap() = state_with(vec![], None);
    let cx = host.cx();
    let out = plugin
        .call_tool("read", json!({ "path": "a.txt" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("workspace_use()"),
        "{}",
        out.content.text
    );

    // An unknown qualifier names the available workspaces and the `./` escape.
    *host.state.lock().unwrap() = state_with(vec![ws("proj", "/nowhere")], Some("proj"));
    let out = plugin
        .call_tool("read", json!({ "path": "ghost:a.txt" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("unknown workspace `ghost`"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("available: proj, @rumu")
            && out.content.text.contains("`./ghost:a.txt`"),
        "{}",
        out.content.text
    );

    // `./a:b.txt` defeats the qualifier syntax and reads a literal filename.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a:b.txt"), "hi").unwrap();
    *host.state.lock().unwrap() =
        state_with(vec![ws("proj", dir.path().to_str().unwrap())], Some("proj"));
    let out = plugin
        .call_tool("read", json!({ "path": "./a:b.txt" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "hi", "{}", out.content.text);
}

/// A Windows-typed workspace (built on any host: the style is explicit, not
/// guessed from the string) resolves both separators and displays with `/`.
#[test]
fn windows_typed_workspaces_resolve_and_display() {
    use rumu_core::{Utf8TypedPathBuf, resolve_state};

    let win = Workspace {
        name: "proj".into(),
        ssh: None,
        root: Utf8TypedPathBuf::from_windows(r"C:\Users\me\proj"),
        read_only: false,
    };
    let state = state_with(vec![win.clone()], Some("proj"));

    let addr = resolve_state(&state, r"src\a.rs").unwrap();
    assert_eq!(addr.abs.as_str(), r"C:\Users\me\proj\src\a.rs");
    assert_eq!(
        rumu_core::display(&addr.ws, win.root.to_path(), addr.abs.to_path()),
        "proj:src/a.rs"
    );
    // `/` and `..` normalize in the workspace's style too.
    assert_eq!(
        resolve_state(&state, "src/../b.rs").unwrap().abs.as_str(),
        r"C:\Users\me\proj\b.rs"
    );
    // `C:\…` is a plain path: a one-letter qualifier is never a workspace.
    let outside = resolve_state(&state, r"C:\Windows\x").unwrap();
    assert_eq!(outside.abs.as_str(), r"C:\Windows\x");
    assert_eq!(
        rumu_core::display(&outside.ws, win.root.to_path(), outside.abs.to_path()),
        r"proj:C:\Windows\x"
    );

    // ssh workspaces stay POSIX.
    let app = Workspace {
        name: "app".into(),
        ssh: Some("deploy@prod".into()),
        root: Utf8TypedPathBuf::from_unix("/srv/app"),
        read_only: false,
    };
    let state = state_with(vec![app], Some("app"));
    let addr = resolve_state(&state, "src/a.rs").unwrap();
    assert_eq!(addr.abs.as_str(), "/srv/app/src/a.rs");
}

/// Absolute paths and `..` resolve outside the root and just work on the
/// machine.
#[tokio::test]
async fn absolute_paths_escape_the_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("inside")).unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);

    // A relative `..` normalizes outside the root.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "../outside.txt", "content": "out" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        std::fs::read_to_string(dir.path().join("../outside.txt").canonicalize().unwrap())
            .unwrap()
            .contains("out")
    );
    assert!(
        out.content.text.contains("wrote proj:/"),
        "outside the root displays absolute: {}",
        out.content.text
    );

    // A machine-absolute path is used as-is, and displays `proj:/abs/...`.
    let abs = dir.path().join("inside/x.txt");
    let abs_str = abs.to_str().unwrap();
    let out = plugin
        .call_tool("write", json!({ "path": abs_str, "content": "abs" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let out = plugin
        .call_tool("read", json!({ "path": abs_str }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "abs");

    // list/edit work the same way.
    let out = plugin
        .call_tool("list", json!({ "path": abs_str }), &cx)
        .await
        .unwrap();
    assert!(out.is_error, "listing a file: {}", out.content.text);
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": abs_str, "old_string": "abs", "new_string": "edited" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let out = plugin
        .call_tool("read", json!({ "path": abs_str }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "edited");
}

// ---------- file tools over the local backend ----------

#[tokio::test]
async fn edit_refuses_non_utf8_files() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);
    let path = dir.path().join("bin.dat");
    std::fs::write(&path, [0xff, 0x00, 0x01]).unwrap();

    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "bin.dat", "old_string": "x", "new_string": "y" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("not a UTF-8 text file; use bash"),
        "{}",
        out.content.text
    );
    // The file is left untouched.
    assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0x00, 0x01]);
}

/// `write`/`edit` results carry the UI's diff payload; failures never do.
#[tokio::test]
async fn write_and_edit_carry_the_ui_patch() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);

    // A write to a new file is a create: no hunks (the UI renders the args).
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "a.txt", "content": "one\ntwo\n" }),
            &cx,
        )
        .await
        .unwrap();
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Create);
    assert!(patch.hunks.is_none());

    // An overwrite is an update diffed against the previous content.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "a.txt", "content": "one\nTWO\n" }),
            &cx,
        )
        .await
        .unwrap();
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Update);
    assert_eq!(patch.hunks.unwrap()[0].lines, [" one", "-two", "+TWO"]);

    // An edit diffs its before/after texts.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "a.txt", "old_string": "one", "new_string": "ONE\n1" }),
            &cx,
        )
        .await
        .unwrap();
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Update);
    assert_eq!(
        patch.hunks.unwrap()[0].lines,
        ["-one", "+ONE", "+1", " TWO"]
    );

    // A failed call never gets one.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "a.txt", "old_string": "nope", "new_string": "x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(out.content.data.is_none());
}

/// Reverting a span puts back what its `write`/`edit` calls changed: a
/// created file is removed, an edited one restored (an older patch naming no
/// file found by its call's path), and one changed by hand since is left
/// alone.
#[tokio::test]
async fn revert_undoes_the_recorded_writes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("b.txt"), "b1\nb2\nb3\n").unwrap();
    std::fs::write(dir.path().join("c.txt"), "c1\nc2\n").unwrap();
    let (plugin, host, cx, _home) = setup(dir.path(), false);

    let calls = [
        (
            "w1",
            "write",
            json!({ "path": "a.txt", "content": "new\n" }),
        ),
        (
            "e1",
            "edit",
            json!({ "path": "b.txt", "old_string": "b2", "new_string": "B2" }),
        ),
        (
            "e2",
            "edit",
            json!({ "path": "b.txt", "old_string": "b3", "new_string": "B3\nB4" }),
        ),
        (
            "w2",
            "write",
            json!({ "path": "c.txt", "content": "c1\nC2\n" }),
        ),
        ("x1", "bash", json!({ "command": "true" })),
    ];
    let user = NodeBody::User {
        content: "do it".into(),
    };
    let mut nodes = vec![sess_node("s", "u", None, 1, user)];
    nodes.push(sess_node(
        "s",
        "a",
        Some("u"),
        2,
        NodeBody::Assistant {
            content: "".into(),
            reasoning: vec![],
            stats: Default::default(),
            reasoning_reset: false,
            tool_calls: calls
                .iter()
                .map(|(id, name, args)| rumu_core::ToolCall::new(*id, *name, args.clone()))
                .collect(),
            model: "m".into(),
            usage: Default::default(),
        },
    ));
    for (i, (id, name, args)) in calls.iter().enumerate() {
        let mut out = plugin.call_tool(name, args.clone(), &cx).await.unwrap();
        assert!(!out.is_error, "{name}: {}", out.content.text);
        flush_output(&host, id, &out);
        // A patch from before they named their file: found by its path.
        if *id == "e1" {
            let data = out.content.data.as_mut().unwrap();
            assert!(data.data.as_object_mut().unwrap().remove("file").is_some());
        }
        nodes.push(sess_node(
            "s",
            &format!("r{i}"),
            None,
            3 + i as i64,
            NodeBody::ToolResult {
                call_id: (*id).into(),
                content: out.content,
                is_error: false,
                invoke: None,
            },
        ));
    }
    // The user edits c.txt's changed line by hand afterwards.
    std::fs::write(dir.path().join("c.txt"), "c1\nmine\n").unwrap();

    let state = host.state.lock().unwrap().clone();
    let plan = revert_plan(&state, &nodes).await;
    let actions: Vec<(&str, RevertAction)> = plan
        .files
        .iter()
        .map(|f| (f.label.as_str(), f.action))
        .collect();
    assert_eq!(
        actions,
        [
            ("proj:a.txt", RevertAction::Delete),
            ("proj:b.txt", RevertAction::Restore),
            ("proj:c.txt", RevertAction::Changed),
        ]
    );
    assert_eq!(plan.commands, ["true"]);

    let every: Vec<rumu_core::FileRef> = plan
        .files
        .iter()
        .map(|f| rumu_core::FileRef {
            ws: f.ws.clone(),
            path: f.path.clone(),
        })
        .collect();
    let (_, done) = revert_files(&state, &nodes, &every).await;
    assert_eq!(done.len(), 2);
    assert!(done.iter().all(|f| f.error.is_none()));
    assert!(!dir.path().join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "b1\nb2\nb3\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("c.txt")).unwrap(),
        "c1\nmine\n"
    );

    let note = revert_note("do it", false, &plan, &done).unwrap();
    assert!(note.contains("restored: proj:b.txt"), "{note}");
    assert!(note.contains("deleted: proj:a.txt"), "{note}");
    assert!(note.contains("left as they are: proj:c.txt"), "{note}");
    assert!(note.contains("bash"), "{note}");

    // Recorded, the revert is not planned again: only the file changed by
    // hand is left.
    let marker = rumu_core::FilesReverted {
        since: 1,
        files: done
            .iter()
            .map(|f| rumu_core::FileRef {
                ws: f.ws.clone(),
                path: f.path.clone(),
            })
            .collect(),
    };
    nodes.push(sess_node(
        "s",
        "n",
        None,
        100,
        NodeBody::Note {
            source: "system".into(),
            content: rumu_core::Content::note_with(note, &marker),
        },
    ));
    let again = revert_plan(&state, &nodes).await;
    assert_eq!(again.files.len(), 1);
    assert_eq!(again.files[0].action, RevertAction::Changed);
}

/// `delete` removes files only, keeping the content in its patch, and a
/// revert puts the file back; read-only workspaces refuse it.
#[tokio::test]
async fn delete_removes_files_and_a_revert_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a1\na2\n").unwrap();
    std::fs::create_dir(dir.path().join("d")).unwrap();
    let (plugin, host, cx, _home) = setup(dir.path(), false);
    let args = json!({ "path": "a.txt" });

    let deleted = plugin.call_tool("delete", args.clone(), &cx).await.unwrap();
    assert!(!deleted.is_error, "{}", deleted.content.text);
    assert!(
        deleted.content.text.starts_with("deleted "),
        "{}",
        deleted.content.text
    );
    assert!(!dir.path().join("a.txt").exists());
    let patch = deleted.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Delete);
    assert!(patch.file.is_some());
    let removed: Vec<&str> = patch.hunks.as_ref().unwrap()[0]
        .lines
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(removed, ["-a1", "-a2"]);

    let again = plugin.call_tool("delete", args.clone(), &cx).await.unwrap();
    assert!(again.is_error && again.content.text.contains("no such file"));
    let out = plugin
        .call_tool("delete", json!({ "path": "d" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("is a directory"),
        "{}",
        out.content.text
    );
    assert!(dir.path().join("d").is_dir());

    let call = rumu_core::ToolCall::new("d1", "delete", args);
    let nodes = vec![
        sess_node(
            "s",
            "u",
            None,
            1,
            NodeBody::User {
                content: "go".into(),
            },
        ),
        sess_node(
            "s",
            "a",
            Some("u"),
            2,
            NodeBody::Assistant {
                content: "".into(),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![call],
                model: "m".into(),
                usage: Default::default(),
            },
        ),
        sess_node(
            "s",
            "r",
            None,
            3,
            NodeBody::ToolResult {
                call_id: "d1".into(),
                content: deleted.content,
                is_error: false,
                invoke: None,
            },
        ),
    ];
    let state = host.state.lock().unwrap().clone();
    let plan = revert_plan(&state, &nodes).await;
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].action, RevertAction::Restore);
    let every = [rumu_core::FileRef {
        ws: plan.files[0].ws.clone(),
        path: plan.files[0].path.clone(),
    }];
    let (_, done) = revert_files(&state, &nodes, &every).await;
    assert!(done.iter().all(|f| f.error.is_none()), "{done:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "a1\na2\n"
    );

    let ro = tempfile::tempdir().unwrap();
    std::fs::write(ro.path().join("k.txt"), "keep\n").unwrap();
    let (plugin, _host, cx, _home) = setup(ro.path(), true);
    let out = plugin
        .call_tool("delete", json!({ "path": "k.txt" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("read-only"));
    assert!(ro.path().join("k.txt").exists());
}

#[tokio::test]
async fn read_write_edit_list_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);

    // `write` never creates directories: a missing one is the model's error.
    let write = json!({ "path": "a/b.txt", "content": "l1\nl2\nl3\n" });
    let out = plugin.call_tool("write", write.clone(), &cx).await.unwrap();
    assert!(out.is_error);
    assert!(
        out.content.text.contains("its directory does not exist"),
        "{}",
        out.content.text
    );
    assert!(!dir.path().join("a").exists());
    // `@scratch/…` for `@scratch:…` gets pointed at the colon form.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@scratch/x.txt", "content": "" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.content.text.contains("did you mean `@scratch:x.txt`?"),
        "{}",
        out.content.text
    );
    // …and so does a bare `@scratch` given to `list`.
    let out = plugin
        .call_tool("list", json!({ "path": "@scratch" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("did you mean `@scratch:`?"),
        "{}",
        out.content.text
    );

    std::fs::create_dir(dir.path().join("a")).unwrap();
    let out = plugin.call_tool("write", write, &cx).await.unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.starts_with("wrote proj:a/b.txt (9 bytes)"),
        "{}",
        out.content.text
    );

    // Raw read: no prefix, no line numbers.
    let out = plugin
        .call_tool("read", json!({ "path": "a/b.txt" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "l1\nl2\nl3\n");

    // offset = 1-based start line, limit = line count.
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": "a/b.txt", "offset": 2, "limit": 2 }),
            &cx,
        )
        .await
        .unwrap();
    assert_eq!(out.content.text, "l2\nl3");
    let out = plugin
        .call_tool("read", json!({ "path": "a/b.txt", "offset": 0 }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("1-based"),
        "{}",
        out.content.text
    );

    // A trailing `#L<a>-L<b>` anchor sets offset/limit when they are omitted.
    let out = plugin
        .call_tool("read", json!({ "path": "a/b.txt#L2-L3" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "l2\nl3", "{}", out.content.text);
    // A single `#L<n>` sets only the offset.
    let out = plugin
        .call_tool("read", json!({ "path": "a/b.txt#L2" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "l2\nl3", "{}", out.content.text);
    // An explicit offset wins over the anchor.
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": "a/b.txt#L2", "offset": 3, "limit": 1 }),
            &cx,
        )
        .await
        .unwrap();
    assert_eq!(out.content.text, "l3", "{}", out.content.text);

    // edit: exact replacement semantics.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "a/b.txt", "old_string": "l2", "new_string": "L2" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        !out.is_error && out.content.text.contains("1 replacement"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("read", json!({ "path": "a/b.txt" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "l1\nL2\nl3\n");
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "a/b.txt", "old_string": "l", "new_string": "x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("replace_all"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "a/b.txt", "old_string": "zzz", "new_string": "x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("not found"),
        "{}",
        out.content.text
    );

    // list: dirs trailing /, prefixed output; `proj:` (empty rest) = root.
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert_eq!(out.content.text, "a/");
    let out = plugin
        .call_tool("list", json!({ "path": "proj:" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "a/");
    let out = plugin
        .call_tool("list", json!({ "path": "a" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "b.txt");

    let out = plugin
        .call_tool("read", json!({ "path": "missing.txt" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.starts_with("read proj:missing.txt: "),
        "{}",
        out.content.text
    );
}

/// Reading an image: the bytes are ingested into `<home>/blobs` and come
/// back as one attachment beside a header line; `offset`/`limit` are
/// refused, undecodable bytes are a model-facing error, and text reads of
/// the same workspace are unchanged.
#[tokio::test]
async fn image_read_ingests_and_attaches() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, home) = setup(dir.path(), false);
    std::fs::create_dir_all(dir.path().join("img")).unwrap();
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(40, 30, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    }))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .unwrap();
    std::fs::write(dir.path().join("img/demo.png"), &png).unwrap();

    let out = plugin
        .call_tool("read", json!({ "path": "img/demo.png" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content
            .text
            .starts_with("image proj:img/demo.png (40×30, ")
            && out.content.text.ends_with(" bytes stored)"),
        "{}",
        out.content.text
    );
    assert_eq!(out.content.attachments.len(), 1);
    let a = &out.content.attachments[0];
    assert_eq!((a.width, a.height), (40, 30));
    assert_eq!(a.mime, "image/png");
    assert_eq!(a.name.as_deref(), Some("demo.png"));
    // The blob exists and the attachment describes it.
    let blob = std::fs::read(home.path().join("blobs").join(&a.hash)).unwrap();
    assert_eq!(blob.len() as u64, a.size);

    // `offset`/`limit` have no lines to slice and are refused.
    let out = plugin
        .call_tool("read", json!({ "path": "img/demo.png", "offset": 1 }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("do not apply to image reads"),
        "{}",
        out.content.text
    );

    // Bytes that sniff as PNG but do not decode fail like other reads.
    std::fs::write(dir.path().join("img/broken.png"), b"\x89PNG\r\n\x1a\nnope").unwrap();
    let out = plugin
        .call_tool("read", json!({ "path": "img/broken.png" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.starts_with("read proj:img/broken.png: "),
        "{}",
        out.content.text
    );

    // A text read still returns raw text with no attachments.
    std::fs::write(dir.path().join("note.txt"), "plain").unwrap();
    let out = plugin
        .call_tool("read", json!({ "path": "note.txt" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "plain");
    assert!(out.content.attachments.is_empty());
}

#[tokio::test]
async fn show_snapshots_a_file_as_an_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, home) = setup(dir.path(), true);
    std::fs::write(dir.path().join("page.html"), "<p>v1</p>").unwrap();

    let out = plugin
        .call_tool(
            "show",
            json!({ "path": "page.html", "title": " Demo " }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(
        out.content.text,
        "showed proj:page.html to the user (9 bytes)"
    );
    let a: rumu_core::Artifact = out.content.data.as_ref().unwrap().decode().unwrap();
    assert_eq!(a.path, "proj:page.html");
    assert_eq!(a.title, "Demo");
    assert_eq!(a.kind, rumu_core::ArtifactKind::Html);
    assert_eq!(a.size, 9);
    // The snapshot outlives the file (read-only workspaces can show too).
    std::fs::remove_file(dir.path().join("page.html")).unwrap();
    let blob = std::fs::read(home.path().join("blobs").join(&a.hash)).unwrap();
    assert_eq!(blob, b"<p>v1</p>");

    // The title defaults to the file name; the extension picks the kind.
    std::fs::write(dir.path().join("notes.MD"), "# hi").unwrap();
    let out = plugin
        .call_tool("show", json!({ "path": "notes.MD" }), &cx)
        .await
        .unwrap();
    let a: rumu_core::Artifact = out.content.data.as_ref().unwrap().decode().unwrap();
    assert_eq!(
        (a.title.as_str(), a.kind),
        ("notes.MD", rumu_core::ArtifactKind::Markdown)
    );

    // Media display or play as themselves, other bytes download; non-UTF-8
    // text extensions are just files.
    for (name, bytes, kind, mime) in [
        (
            "shot.PNG",
            &b"\x89PNG"[..],
            rumu_core::ArtifactKind::Image,
            Some("image/png"),
        ),
        (
            "clip.mp4",
            b"\0\0\0\x18ftyp",
            rumu_core::ArtifactKind::Video,
            Some("video/mp4"),
        ),
        (
            "song.mp3",
            b"ID3",
            rumu_core::ArtifactKind::Audio,
            Some("audio/mpeg"),
        ),
        (
            "paper.pdf",
            b"%PDF-1.7",
            rumu_core::ArtifactKind::Pdf,
            Some("application/pdf"),
        ),
        (
            "bin.dat",
            &[0xff, 0xfe, 0x00],
            rumu_core::ArtifactKind::File,
            None,
        ),
        ("odd.md", &[0xff], rumu_core::ArtifactKind::File, None),
    ] {
        std::fs::write(dir.path().join(name), bytes).unwrap();
        let out = plugin
            .call_tool("show", json!({ "path": name }), &cx)
            .await
            .unwrap();
        assert!(!out.is_error, "{name}: {}", out.content.text);
        let a: rumu_core::Artifact = out.content.data.as_ref().unwrap().decode().unwrap();
        assert_eq!((a.kind, a.mime.as_deref()), (kind, mime), "{name}");
    }

    // Missing files and `@rumu` are refused.
    for (path, needle) in [
        ("gone.txt", "show proj:gone.txt: "),
        ("@rumu:/cards/self/x", "takes a workspace file"),
    ] {
        let out = plugin
            .call_tool("show", json!({ "path": path }), &cx)
            .await
            .unwrap();
        assert!(
            out.is_error && out.content.text.contains(needle),
            "{path}: {}",
            out.content.text
        );
        assert!(out.content.data.is_none());
    }
}

#[tokio::test]
async fn read_only_and_bash_behaviour() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), true);

    let out = plugin
        .call_tool("write", json!({ "path": "x.txt", "content": "x" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("read-only"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "x.txt", "old_string": "a", "new_string": "b" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("read-only"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("bash", json!({ "command": "true" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("read-only"),
        "{}",
        out.content.text
    );

    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);

    let out = plugin
        .call_tool("bash", json!({ "command": "echo out; echo err >&2" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.starts_with("exit code: 0\n"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("\nout\nerr"),
        "{}",
        out.content.text
    );

    let out = plugin
        .call_tool("bash", json!({ "command": "exit 3" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(
        out.content.text.contains("exit code: 3"),
        "{}",
        out.content.text
    );

    // The tool returns its whole output; the kernel caps what the model sees.
    let out = plugin
        .call_tool("bash", json!({ "command": "seq 1 8000" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.ends_with("8000\n"),
        "{} chars",
        out.content.text.len()
    );
    let capped = out.cap();
    assert!(capped.content.text.contains("[truncated"));
    assert!(capped.content.text.chars().count() < 22_000);

    // Timeout: capped `timeout` argument applies.
    let out = plugin
        .call_tool("bash", json!({ "command": "sleep 5", "timeout": 1 }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("timed out after 1s"),
        "{}",
        out.content.text
    );

    // `bash`'s `workspace` argument takes a bare name only.
    let out = plugin
        .call_tool(
            "bash",
            json!({ "command": "true", "workspace": "@rumu" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error
            && out
                .content
                .text
                .contains("the `@rumu` workspace has no bash"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "bash",
            json!({ "command": "true", "workspace": "@rumu://workspace/proj" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("unknown workspace"),
        "URI forms are gone: {}",
        out.content.text
    );
}

/// No project workspace: `bash` runs in the session's `@scratch` directory,
/// which the first use creates, and files it writes are readable through
/// `@scratch:`.
#[tokio::test]
async fn bash_runs_in_the_scratch_workspace_and_creates_it() {
    let home = tempfile::tempdir().unwrap();
    let plugin = deck_plugin(home.path());
    let host = FakeRuntime::arc();
    let scratch = home.path().join("scratch/s1");
    assert!(!scratch.exists(), "the scratch directory starts absent");
    *host.state.lock().unwrap() = state_with(
        vec![Workspace {
            name: SCRATCH_WORKSPACE.into(),
            ssh: None,
            root: scratch.to_string_lossy().into_owned().into(),
            read_only: false,
        }],
        Some(SCRATCH_WORKSPACE),
    );
    let cx = host.cx();

    let out = plugin
        .call_tool("bash", json!({ "command": "echo hi > note.txt" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(scratch.exists(), "the first use creates the directory");
    assert_eq!(
        std::fs::read_to_string(scratch.join("note.txt")).unwrap(),
        "hi\n"
    );

    let out = plugin
        .call_tool("read", json!({ "path": "@scratch:note.txt" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(out.content.text, "hi\n");

    // `@rumu` is still the one workspace without bash.
    let out = plugin
        .call_tool(
            "bash",
            json!({ "command": "true", "workspace": "@rumu" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error
            && out
                .content
                .text
                .contains("the `@rumu` workspace has no bash"),
        "{}",
        out.content.text
    );
}

// ---------- workspace_use ----------

#[tokio::test]
async fn workspace_use_lists_and_switches() {
    let home = tempfile::tempdir().unwrap();
    let prod = Workspace {
        name: "prod".into(),
        ssh: Some("deploy@prod".into()),
        root: "/srv/app".into(),
        read_only: true,
    };
    let plugin = deck_plugin(home.path());
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = state_with(vec![ws("proj", "/p/rumu"), prod], Some("proj"));
    let cx = host.cx();

    // Without `name`: just the listing, and no marker is written.
    let out = plugin
        .call_tool("workspace_use", json!({}), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let lines: Vec<&str> = out.content.text.lines().collect();
    assert!(
        lines[0].starts_with("* proj local /p/rumu rw"),
        "{}",
        lines[0]
    );
    assert!(
        lines[1].starts_with("  prod ssh:deploy@prod /srv/app ro"),
        "{}",
        lines[1]
    );
    assert!(
        lines[2].starts_with("  @rumu rumu's own data: @rumu:/cards"),
        "{}",
        lines[2]
    );
    assert!(out.effects.is_empty(), "no marker");

    // With `name`: the marker is returned, the listing reflects the default.
    let out = plugin
        .call_tool("workspace_use", json!({ "name": "prod" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.starts_with("default workspace set"),
        "{}",
        out.content.text
    );
    assert!(
        out.content
            .text
            .lines()
            .nth(2)
            .unwrap()
            .starts_with("* prod"),
        "{}",
        out.content.text
    );
    assert_eq!(out.effects.len(), 1);
    let content = &out.effects[0];
    let tagged = content.data.as_ref().expect("workspace.use payload");
    assert_eq!(tagged.tag, WorkspaceUse::TAG);
    let payload: WorkspaceUse = tagged.decode().expect("workspace.use payload");
    assert_eq!(payload.name, "prod");

    // `@rumu` is a valid target too (and becomes the starred line).
    let out = plugin
        .call_tool("workspace_use", json!({ "name": "@rumu" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let starred = out
        .content
        .text
        .lines()
        .find(|l| l.starts_with('*'))
        .expect("a starred line");
    assert!(starred.contains("rumu's own data"), "{starred}");

    // Unknown names error with the available list.
    let out = plugin
        .call_tool("workspace_use", json!({ "name": "ghost" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("unknown workspace `ghost`"),
        "{}",
        out.content.text
    );
}

// ---------- AGENTS.md ----------

#[tokio::test]
async fn agents_md_injected_once_per_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "project rules").unwrap();
    let (plugin, host, cx, _home) = setup(dir.path(), false);

    let out = plugin
        .call_tool("read", json!({ "path": "a" }), &cx)
        .await
        .unwrap();
    let first = out.effects[0].clone();
    let agents: super::agents::AgentsNote = first.decode().expect("workspace.agents payload");
    assert_eq!(agents.ws, "proj");
    assert_eq!(agents.file, "AGENTS.md");
    assert!(
        first.text.contains("AGENTS.md") && first.text.contains("project rules"),
        "{}",
        first.text
    );
    assert!(first.text.contains("(workspace `proj`)"), "{}", first.text);
    assert!(first.text.contains("\n\nproject rules"), "{}", first.text);

    // Already on the path (same hash) → not injected again.
    host.nodes.lock().unwrap().push(Node {
        id: rumu_core::NodeId::new(),
        parent: None,
        session: Default::default(),
        at: 0,
        dur_ms: None,
        body: NodeBody::Note {
            source: "workspace".into(),
            content: first.clone(),
        },
    });
    let cx = host.cx();
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert!(
        out.effects.is_empty(),
        "already injected: {:?}",
        out.effects
    );

    // CLAUDE.md is the fallback; an empty dir injects nothing.
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(dir2.path().join("CLAUDE.md"), "claude rules").unwrap();
    let (plugin2, _host2, cx2, _home2) = setup(dir2.path(), false);
    let out = plugin2.call_tool("list", json!({}), &cx2).await.unwrap();
    assert!(
        out.effects.iter().any(|e| {
            e.decode::<super::agents::AgentsNote>()
                .is_some_and(|a| a.file == "CLAUDE.md")
                && e.text.contains("claude rules")
        }),
        "{:?}",
        out.effects
    );

    let dir3 = tempfile::tempdir().unwrap();
    let (plugin3, _host3, cx3, _home3) = setup(dir3.path(), false);
    let out = plugin3.call_tool("list", json!({}), &cx3).await.unwrap();
    assert!(out.effects.is_empty());

    // An empty instructions file is skipped entirely.
    let dir4 = tempfile::tempdir().unwrap();
    std::fs::write(dir4.path().join("AGENTS.md"), "  \n").unwrap();
    let (plugin4, _host4, cx4, _home4) = setup(dir4.path(), false);
    let out = plugin4.call_tool("list", json!({}), &cx4).await.unwrap();
    assert!(out.effects.is_empty());
}

/// A second tool call in the same batch sees the first call's queued agents
/// note (`cx.path` includes queued effects), and a changed file is injected
/// once more with an "(updated)" header.
#[tokio::test]
async fn agents_updated_on_hash_change_and_once_per_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "v1 rules").unwrap();
    let (plugin, host, _cx, _home) = setup(dir.path(), false);

    let cx = host.cx();
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    let first = out.effects[0].clone();
    // The kernel's safe point writes the effect before the next call.
    flush_output(
        &host,
        "call-1",
        &ToolOutput::ok("ok").with_effects(vec![first.clone()]),
    );

    // Same batch, second call: no duplicate.
    let cx = host.cx();
    let out = plugin
        .call_tool("read", json!({ "path": "x" }), &cx)
        .await
        .unwrap();
    assert!(out.effects.is_empty(), "{:?}", out.effects);

    // The file changes → one re-injection with an updated header.
    std::fs::write(dir.path().join("AGENTS.md"), "v2 rules").unwrap();
    let cx = host.cx();
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert_eq!(out.effects.len(), 1, "{:?}", out.effects);
    let second = &out.effects[0];
    assert!(second.text.contains("(updated)"), "{}", second.text);
    assert!(second.text.contains("v2 rules"), "{}", second.text);
    let old: super::agents::AgentsNote = first.decode().unwrap();
    let new: super::agents::AgentsNote = second.decode().unwrap();
    assert_ne!(old.hash, new.hash);
}

// ---------- the `@rumu` workspace: /cards ----------
fn card_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(dir.path());
    dir
}

fn card_plugin_for(home: &Path) -> (WorkspacePlugin, std::sync::Arc<FakeRuntime>, rumu_core::Cx) {
    let plugin = deck_plugin(home);
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = state_with(vec![], Some(RUMU_WORKSPACE));
    let cx = host.cx();
    (plugin, host, cx)
}

/// `@rumu:/cards/self/x` ≡ `@rumu:/cards/self/x.md`; with `@rumu` as the branch default,
/// `/cards/x` and `cards/x` work too.
#[tokio::test]
async fn card_paths_are_equivalent_in_every_form() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "m", "---\ntitle: Old\n---\nbody v1\n");
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let mut effects = Vec::new();
    for path in [
        "@rumu:/cards/self/m",
        "@rumu:/cards/self/m.md",
        "@rumu:cards/self/m",
        "/cards/self/m",
        "cards/self/m",
    ] {
        let out = plugin
            .call_tool("read", json!({ "path": path }), &cx)
            .await
            .unwrap();
        assert!(!out.is_error, "{path}: {}", out.content.text);
        assert!(
            out.content.text.contains("title: Old") && out.content.text.ends_with("---\nbody v1\n"),
            "{path}: {}",
            out.content.text
        );
        effects.extend(out.effects);
    }
    // Every read records one use marker.
    assert_eq!(effects.len(), 5);
    let payload: usage::CardUse = effects[0].decode().expect("card_use payload");
    assert_eq!(payload.deck, crate::testutil::MAIN_DECK);
    assert_eq!(payload.path, "m");
    assert_eq!(payload.op, usage::CardOp::Read);
}

/// A successful model read bumps the card's usage by one; a link bumps its
/// target.
#[tokio::test]
async fn card_read_bumps_the_usage_index() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "m", "body\n");
    let card = deck_card(home.path(), "m");
    let (plugin, _host, cx) = card_plugin_for(home.path());
    let dir = plugin.decks().get(crate::testutil::MAIN_DECK).unwrap().dir;
    let score = || {
        rumu_cards::usage_index::scores(&dir, rumu_core::now())
            .get(card.id().unwrap())
            .copied()
            .unwrap_or(0.0)
    };
    let before = score();
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/m" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        score() > before,
        "a successful read adds use: {before} -> {}",
        score()
    );
}

/// Other roots and bash on `@rumu` are refused with useful errors.
#[tokio::test]
async fn rumu_workspace_edges() {
    let home = card_home();
    let (plugin, _host, cx) = card_plugin_for(home.path());

    // `list @rumu:` shows the roots.
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert_eq!(
        out.content.text,
        "cards/\nhistory/\nsessions/\nskills/\ntasks/\ntools/\nworkspaces/"
    );

    // Unknown paths name the roots.
    for tool in ["read", "write", "edit", "list"] {
        let args = if tool == "write" {
            json!({ "path": "/foo", "content": "x" })
        } else if tool == "edit" {
            json!({ "path": "/foo", "old_string": "a", "new_string": "b" })
        } else {
            json!({ "path": "/foo" })
        };
        let out = plugin.call_tool(tool, args, &cx).await.unwrap();
        assert!(out.is_error, "{tool}: {}", out.content.text);
        assert!(
            out.content
                .text
                .contains("roots: /cards, /history, /sessions"),
            "{tool}: {}",
            out.content.text
        );
    }

    // `/sessions` is a known, read-only root: writes there are refused.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/sessions/x", "content": "x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("`@rumu:/sessions` is read-only"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "/sessions/s", "old_string": "a", "new_string": "b" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("`@rumu:/sessions` is read-only"),
        "{}",
        out.content.text
    );

    // bash on `@rumu` (explicit or default) is refused.
    for args in [
        json!({ "command": "ls", "workspace": "@rumu" }),
        json!({ "command": "ls" }),
    ] {
        let out = plugin.call_tool("bash", args, &cx).await.unwrap();
        assert!(out.is_error, "{}", out.content.text);
        assert!(
            out.content
                .text
                .contains("the `@rumu` workspace has no bash; pass `workspace`"),
            "{}",
            out.content.text
        );
    }

    // `/sessions` reads and lists answer with the (here empty) session
    // listing; only writes are refused outright.
    for tool in ["read", "list"] {
        let out = plugin
            .call_tool(tool, json!({ "path": "@rumu:/sessions" }), &cx)
            .await
            .unwrap();
        assert!(
            !out.is_error && out.content.text.contains("no sessions yet"),
            "{}",
            out.content.text
        );
    }
}

#[tokio::test]
async fn card_writes_flag_broken_and_mount_path_links() {
    let home = card_home();
    crate::testutil::put_card(
        home.path(),
        "main",
        "dev/loop",
        "---\ndescription: d\n---\nloop\n",
    );
    let (plugin, host, cx) = card_plugin_for(home.path());

    let write = |body: &str| {
        plugin.call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/dev/kernel",
                "content": format!("---\ndescription: d\n---\n{body}\n"),
            }),
            &cx,
        )
    };
    let out = write("[loop](./loop) and [up](../dev/loop.md)")
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(!out.content.text.contains('\n'), "{}", out.content.text);
    flush_output(&host, "call-1", &out);

    let out = write("[gone](./gone), [m](@rumu:/cards/self/dev/loop)")
        .await
        .unwrap();
    assert!(!out.is_error, "the write still lands: {}", out.content.text);
    assert!(
        out.content
            .text
            .contains("name no card in this deck: `./gone`."),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("mount path")
            && out.content.text.contains("`@rumu:/cards/self/dev/loop`"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn card_tools_read_write_validate_and_list() {
    let home = card_home();
    crate::testutil::put_card(
        home.path(),
        "main",
        "m",
        "---\ntitle: Old\ndescription: d\ntags: [a]\nmatch:\n  event: prompt\nowner: x\n---\nbody v1\n",
    );
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/m" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("title: Old"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("owner: x"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.ends_with("---\nbody v1\n"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("tags"),
        "unknown frontmatter keys pass through: {}",
        out.content.text
    );
    // `match:` is ordinary frontmatter now: the editable view shows it.
    assert!(
        out.content.text.contains("match:"),
        "match visible: {}",
        out.content.text
    );
    assert!(
        out.content.text.contains("event: prompt"),
        "{}",
        out.content.text
    );

    // The use marker carries both revs; `rev` ignores metadata.
    let payload: usage::CardUse = out.effects[0].decode().expect("card_use payload");
    assert_eq!(payload.deck, crate::testutil::MAIN_DECK);
    assert_eq!(payload.path, "m");
    assert_eq!(payload.op, usage::CardOp::Read);
    assert_eq!(payload.view_rev, card::view_rev_of(&out.content.text));
    assert_eq!(payload.rev.len(), 8);

    // Unknown id: near-miss suggestions, no marker.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/x" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no card"),
        "{}",
        out.content.text
    );
    assert!(out.effects.is_empty());
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/mm" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("did you mean: self/m"),
        "{}",
        out.content.text
    );
    assert!(out.effects.is_empty());

    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "c", "---\ntitle: C\n---\nbody v1\n");
    let (plugin, host, cx) = card_plugin_for(home.path());

    // Overwrite without a prior read → refused.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/c", "content": "---\ntitle: C\n---\nbody v2\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("read it first"),
        "{}",
        out.content.text
    );
    assert!(
        crate::testutil::card_text(home.path(), "main", "c")
            .unwrap()
            .contains("v1")
    );

    // Read, then the same write passes.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/c" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &out);
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/c", "content": "---\ntitle: C\n---\nbody v2\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("view rev"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.starts_with("wrote cards/self/c.md"),
        "{}",
        out.content.text
    );
    flush_output(&host, "call-1", &out);

    // The write itself records the new view_rev → a chained edit needs no
    // re-read.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/self/c", "old_string": "v2", "new_string": "v3" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        crate::testutil::card_text(home.path(), "main", "c")
            .unwrap()
            .contains("v3")
    );
    // A card edit carries its view diff for the UI, like a file edit.
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Update);
    let lines = &patch.hunks.unwrap()[0].lines;
    assert!(lines.contains(&"-body v2".to_owned()), "{lines:?}");
    assert!(lines.contains(&"+body v3".to_owned()), "{lines:?}");

    // Someone edits the file behind our back → mismatch error.
    crate::testutil::put_card(home.path(), "main", "c", "---\ntitle: C\n---\nbody EVIL\n");
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/self/c", "old_string": "v3", "new_string": "v4" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("changed since you last read"),
        "{}",
        out.content.text
    );
    assert!(out.content.text.contains("re-read"), "{}", out.content.text);
    assert!(
        crate::testutil::card_text(home.path(), "main", "c")
            .unwrap()
            .contains("EVIL")
    );

    // Creating a brand-new card needs no read.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/new/card", "content": "fresh body\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "main", "new/card").is_some());
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Create);

    let home = card_home();
    crate::testutil::put_card(
        home.path(),
        "main",
        "m",
        "---\ntitle: Old\nmatch:\n  event: prompt\n---\nbody\n",
    );
    let (plugin, host, cx) = card_plugin_for(home.path());
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/m" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &out);

    // `match:` and `scope:` are editable like any other frontmatter key.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/m", "content": "---\nmatch:\n  event: prompt\nscope: [proj]\n---\nhacked\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = crate::testutil::card_text(home.path(), "main", "m").unwrap();
    assert!(text.contains("hacked"), "{text}");
    assert!(text.contains("match:"), "{text}");
    assert!(text.contains("scope:"), "{text}");
    flush_output(&host, "call-1", &out);

    // Bad types name the offending field.
    for bad in ["---\ndescription: [x]\n---\nb\n", "---\n- a\n- b\n---\nb\n"] {
        let out = plugin
            .call_tool(
                "write",
                json!({ "path": "@rumu:/cards/self/m", "content": bad }),
                &cx,
            )
            .await
            .unwrap();
        assert!(out.is_error, "{bad}");
    }

    // A clean write keeps the `match:` the view carries. A body change moves
    // `rev` (it is part of the render).
    let before = deck_card(home.path(), "m");
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/m", "content": "---\nowner: me\nmatch:\n  event: prompt\n---\nnew body\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = crate::testutil::card_text(home.path(), "main", "m").unwrap();
    assert!(text.contains("owner: me"), "{text}");
    assert!(text.contains("match:"), "{text}");
    assert!(text.contains("event: prompt"), "{text}");
    assert!(!text.contains("scope:"), "omitted keys are dropped: {text}");
    let after = deck_card(home.path(), "m");
    assert_ne!(before.revision, after.revision, "the body is in the render");
    flush_output(&host, "call-1", &out);

    // A description-only edit keeps `rev` (metadata is invisible to the
    // memory plugin) but moves `view_rev`.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/self/m", "old_string": "owner: me", "new_string": "owner: me\ndescription: added" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let with_desc = deck_card(home.path(), "m");
    assert_eq!(
        after.revision, with_desc.revision,
        "description edits keep `rev`"
    );
    assert_ne!(
        card::view_rev_of(&card::view_of(&after)),
        card::view_rev_of(&card::view_of(&with_desc)),
        "but move `view_rev`"
    );
    flush_output(&host, "call-1", &out);

    // A write whose view omits `match:` removes it (nothing is force-merged).
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/m", "content": "---\nowner: me\n---\nplain\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = crate::testutil::card_text(home.path(), "main", "m").unwrap();
    assert!(!text.contains("match:"), "omitted match is dropped: {text}");

    let home = card_home();
    crate::testutil::put_card(
        home.path(),
        "main",
        "x",
        "---\ndescription: dx\n---\nsecret body\n",
    );
    crate::testutil::put_card(home.path(), "main", "dev/y", "plain\n");
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/cards" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("@rumu:/cards/self/dev/y — plain"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("@rumu:/cards/self/x — dx"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("secret body"),
        "{}",
        out.content.text
    );

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/cards/self/dev" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("@rumu:/cards/self/dev/y"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("@rumu:/cards/self/x — X"),
        "{}",
        out.content.text
    );
}

// ---------- the `@rumu` workspace: /sessions ----------

/// A staged session: root marker R, then user U1 → assistant A1 (with a
/// `bash` call) → tool_result T1 → note N1 → marker M1 on `main`; an
/// `agent` branch forked at A1 with its answer E1 and final reply A2.
fn staged_session() -> (rumu_core::SessionInfo, Vec<Node>, Vec<rumu_core::Branch>) {
    let sid = rumu_core::SessionId::from("01STESTSSESSION0000000000000");
    let mk = |id: &str, parent: Option<&str>, at: i64, body: NodeBody| Node {
        id: rumu_core::NodeId::from(id),
        parent: parent.map(rumu_core::NodeId::from),
        session: sid.clone(),
        at,
        dur_ms: None,
        body,
    };
    let nodes = vec![
        mk(
            "R",
            None,
            1000,
            NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
            },
        ),
        mk(
            "U1",
            Some("R"),
            2000,
            NodeBody::User {
                content: rumu_core::Content::text("list the files"),
            },
        ),
        mk(
            "A1",
            Some("U1"),
            3000,
            NodeBody::Assistant {
                content: rumu_core::Content::text("checking"),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall::new(
                    "c1",
                    "bash",
                    json!({ "command": "ls -la" }),
                )],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
        mk(
            "T1",
            Some("A1"),
            4000,
            NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text("file-a\nfile-b"),
                is_error: false,
                invoke: None,
            },
        ),
        mk(
            "N1",
            Some("T1"),
            5000,
            NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content::text("# x\ncard body"),
            },
        ),
        mk(
            "M1",
            Some("N1"),
            6000,
            NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content {
                    text: String::new(),
                    attachments: Vec::new(),
                    data: Some(Tagged {
                        tag: "tag".into(),
                        v: 1,
                        data: json!({"name": "infra"}),
                    }),
                },
            },
        ),
        mk(
            "E1",
            Some("A1"),
            7000,
            NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text(
                    "You are now a sub-agent. Goal: find the config file\nDo the work, then reply with a concise summary — a reply without tool calls ends the sub-agent. Nothing you do here is visible to the main branch except that final reply.",
                ),
                is_error: false,
                invoke: None,
            },
        ),
        mk(
            "A2",
            Some("E1"),
            8000,
            NodeBody::Assistant {
                content: rumu_core::Content::text("config lives in /etc/rumu"),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
    ];
    let main = rumu_core::Branch {
        id: rumu_core::BranchId::from("BMAIN"),
        session: sid.clone(),
        head: rumu_core::NodeId::from("M1"),
        forked_from: None,
        kind: rumu_core::BranchKind::Main,
        status: rumu_core::BranchStatus::Active,
        created_at: 1000,
    };
    let agent = rumu_core::Branch {
        id: rumu_core::BranchId::from("BEXP"),
        session: sid.clone(),
        head: rumu_core::NodeId::from("A2"),
        forked_from: Some(rumu_core::NodeId::from("A1")),
        kind: rumu_core::BranchKind::Agent {
            task: "find the config file".into(),
            parent_branch: main.id.clone(),
            call_id: "call-1".into(),
            via_plugin: false,
            inherit: Default::default(),
        },
        status: rumu_core::BranchStatus::Done,
        created_at: 6500,
    };
    let info = rumu_core::SessionInfo {
        id: sid,
        title: "config hunting".into(),
        active_branch: main.id.clone(),
        created_at: 1000,
        updated_at: 8000,
        touched_at: 8000,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    (info, nodes, vec![main, agent])
}

fn session_host(
    info: rumu_core::SessionInfo,
    nodes: Vec<Node>,
    branches: Vec<rumu_core::Branch>,
) -> std::sync::Arc<FakeRuntime> {
    let host = FakeRuntime::arc();
    host.sessions.lock().unwrap().push(info.clone());
    host.session_trees
        .lock()
        .unwrap()
        .push((info.id.clone(), info, nodes, branches));
    host
}

fn session_plugin() -> WorkspacePlugin {
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    // Leak the tempdir: the plugin only reads RUMU_HOME lazily and the tests
    // finish long before /tmp cleanup matters.
    let home = home.keep();
    deck_plugin(&home)
}

/// Parse a rendered timestamp and assert it denotes `ms` since the epoch.
/// The rendering is in the local zone, so the *instant* is what's comparable
/// across machines.
fn assert_stamp(token: &str, ms: i64) {
    let stamp: jiff::Timestamp = token
        .parse()
        .unwrap_or_else(|e| panic!("`{token}` is not an RFC 3339 timestamp: {e}"));
    assert_eq!(stamp.as_millisecond(), ms);
}

#[tokio::test]
async fn session_listing_and_overview() {
    let (info, nodes, branches) = staged_session();
    let host = session_host(info, nodes, branches);
    let cx = host.cx();
    let plugin = session_plugin();

    // The listing line: address, updated timestamp, title.
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/sessions" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let line = out.content.text.lines().next().unwrap();
    let mut fields = line.split("  ");
    assert_eq!(
        fields.next(),
        Some("@rumu:/sessions/01STESTSSESSION0000000000000")
    );
    assert_stamp(fields.next().unwrap(), 8000);
    assert_eq!(fields.next(), Some("\"config hunting\""), "{line}");
    assert_eq!(fields.next(), None, "another session is not marked: {line}");

    // The session the call runs in is marked.
    let here = host.cx_for(rumu_core::SessionId::from("01STESTSSESSION0000000000000"));
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/sessions" }), &here)
        .await
        .unwrap();
    let line = out.content.text.lines().next().unwrap();
    assert!(line.ends_with("\"config hunting\"  (current)"), "{line}");

    // The overview: header + one line per branch (created_at order), main
    // first with its own first node (the root), agent with its task.
    for tool in ["read", "list"] {
        let out = plugin
            .call_tool(
                tool,
                json!({ "path": "@rumu:/sessions/01STESTSSESSION0000000000000" }),
                &cx,
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
        let header = out.content.text.lines().next().unwrap();
        assert!(
            header.starts_with(
                "session 01STESTSSESSION0000000000000 — \"config hunting\" · updated "
            ),
            "{header}"
        );
        assert_stamp(header.rsplit_once(" · updated ").unwrap().1, 8000);
        assert_eq!(out.content.text.lines().nth(1), Some("branches:"));
        let main_line = out.content.text.lines().nth(2).unwrap();
        assert!(
            main_line.starts_with(
                "  main     active     @rumu:/sessions/01STESTSSESSION0000000000000/R..M1"
            ),
            "{main_line}"
        );
        let agent_line = out.content.text.lines().nth(3).unwrap();
        assert!(
            agent_line.starts_with("  agent    done       @rumu:/sessions/01STESTSSESSION0000000000000/E1..A2  task: find the config file"),
            "{agent_line}"
        );
    }

    // Unknown sessions error clearly.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/sessions/nosuch" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no session `nosuch`"),
        "{}",
        out.content.text
    );
}

/// Another assistant's sessions are hidden until its deck is mounted; then
/// `list` and `search` show them (naming it), and `assistant` narrows both.
#[tokio::test]
async fn mounted_assistant_sessions_are_visible() {
    const LEDGER: &str = "0193a2f1-0000-7000-8000-00000000b2b2";
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "ledger",
        Some(&format!("id: {LEDGER}\nname: ledger\nassistant: true\n")),
        &[],
    );
    let plugin = deck_plugin(home.path());

    let (info, nodes, branches) = staged_session();
    let host = session_host(info, nodes, branches);
    let (mut other, other_nodes, other_branches) = staged_session();
    other.id = rumu_core::SessionId::from("01SLEDGERSESSION000000000000");
    other.title = "budget".into();
    other.assistant = LEDGER.into();
    host.sessions.lock().unwrap().push(other.clone());
    host.session_trees
        .lock()
        .unwrap()
        .push((other.id.clone(), other, other_nodes, other_branches));
    let cx = host.cx();
    let call = |tool: &'static str, args: serde_json::Value| {
        let (plugin, cx) = (&plugin, &cx);
        async move { plugin.call_tool(tool, args, cx).await.unwrap() }
    };
    let ledger_path = "@rumu:/sessions/01SLEDGERSESSION000000000000";
    let by_ledger = format!("@rumu:/sessions/by-assistant/{LEDGER}");
    let by_main = format!(
        "@rumu:/sessions/by-assistant/{}",
        crate::testutil::MAIN_DECK
    );

    // Not mounted: hidden everywhere, and the filter is refused.
    let out = call("list", json!({ "path": "@rumu:/sessions" })).await;
    assert!(!out.content.text.contains("LEDGER"), "{}", out.content.text);
    let out = call("read", json!({ "path": ledger_path })).await;
    assert!(out.is_error, "{}", out.content.text);
    let out = call("list", json!({ "path": by_ledger })).await;
    assert!(
        out.is_error && out.content.text.contains("no visible sessions"),
        "{}",
        out.content.text
    );

    // Mounted: listed and searchable, named by its deck.
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: LEDGER.into(),
            at: "/cards/ledger".into(),
        });
    let out = call("list", json!({ "path": "@rumu:/sessions" })).await;
    let ledger_line = out
        .content
        .text
        .lines()
        .find(|l| l.starts_with(ledger_path))
        .unwrap_or_else(|| panic!("{}", out.content.text));
    assert!(
        ledger_line.ends_with("\"budget\"  by ledger"),
        "{ledger_line}"
    );
    assert!(
        !out.content
            .text
            .lines()
            .any(|l| l.contains("config hunting") && l.contains(" by ")),
        "{}",
        out.content.text
    );
    let out = call("read", json!({ "path": ledger_path })).await;
    assert!(!out.is_error, "{}", out.content.text);
    let out = call(
        "search",
        json!({ "query": "config", "scope": "@rumu:/sessions" }),
    )
    .await;
    assert!(
        out.content.text.contains(ledger_path),
        "{}",
        out.content.text
    );

    // `by-assistant` lists the visible assistants, own first, and narrows
    // listing and search to one.
    let out = call("list", json!({ "path": "@rumu:/sessions/by-assistant" })).await;
    assert_eq!(
        out.content.text,
        format!(
            "{by_main}  main (you)
{by_ledger}  ledger"
        ),
    );
    let out = call("list", json!({ "path": by_ledger })).await;
    assert_eq!(out.content.text.lines().count(), 1, "{}", out.content.text);
    assert!(
        out.content.text.starts_with(ledger_path),
        "{}",
        out.content.text
    );
    let out = call("search", json!({ "query": "config", "scope": by_main })).await;
    assert!(
        !out.content.text.contains(ledger_path),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("01STESTSSESSION0000000000000"),
        "{}",
        out.content.text
    );

    // Nothing lives below `by-assistant/<id>`.
    let out = call("read", json!({ "path": format!("{by_ledger}/x") })).await;
    assert!(out.is_error, "{}", out.content.text);
}

#[tokio::test]
async fn session_node_and_range_reads() {
    let (info, nodes, branches) = staged_session();
    let host = session_host(info, nodes, branches);
    let cx = host.cx();
    let plugin = session_plugin();
    let base = "@rumu:/sessions/01STESTSSESSION0000000000000";

    // A single node renders fully: the `<node>` element, text, tool call.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/A1") }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(
        out.content.text,
        "<node id=\"A1\" kind=\"assistant\">\nchecking\n\
         <call id=\"c1\" name=\"bash\">{\"command\":\"ls -la\"}</call>\n</node>\n",
        "{}",
        out.content.text
    );

    // Notes carry source; markers are one line; tool results their call id
    // and resolved tool name.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/N1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.starts_with(
            "<node id=\"N1\" kind=\"note\" source=\"memory\">\n# x\ncard body\n</node>\n"
        ),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/M1") }), &cx)
        .await
        .unwrap();
    assert_eq!(
        out.content.text,
        "<node id=\"M1\" kind=\"marker\" tag=\"tag\">{\"name\":\"infra\"}</node>\n",
        "{}",
        out.content.text
    );

    // A range renders a..b inclusive in root→head order.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1..T1") }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let ids: Vec<&str> = out
        .content
        .text
        .lines()
        .filter(|l| l.starts_with("<node "))
        .collect();
    assert_eq!(
        ids,
        [
            "<node id=\"U1\" kind=\"user\">",
            "<node id=\"A1\" kind=\"assistant\">",
            "<node id=\"T1\" kind=\"tool_result\" call=\"c1\" tool=\"bash\">",
        ],
        "{}",
        out.content.text
    );

    // `a` must be `b` or an ancestor of it.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/T1..U1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error
            && out
                .content
                .text
                .contains("`T1` is not `U1` or an ancestor of it"),
        "{}",
        out.content.text
    );
    // A node on another branch is not an ancestor: T1 (main) never appears
    // on A2's chain (agent). (Ranges *do* cross branch points — U1..A2 is
    // valid, since U1 is an ancestor of A2 through the fork.)
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/T1..A2") }), &cx)
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);

    // Unknown node ids error.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/ZZZ") }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no node `ZZZ`"),
        "{}",
        out.content.text
    );

    // offset/limit slice the rendered text by line (1-based, like files).
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("{base}/U1..T1"), "offset": 4, "limit": 2 }),
            &cx,
        )
        .await
        .unwrap();
    assert_eq!(
        out.content.text, "<node id=\"A1\" kind=\"assistant\">\nchecking",
        "{}",
        out.content.text
    );
}

/// Nodes carrying image attachments render their `[image: …]` placeholder
/// (transcripts are text-only): an anchor is replaced in place, an
/// unanchored image follows the text.
#[tokio::test]
async fn session_nodes_show_image_placeholders() {
    let sid = rumu_core::SessionId::from("01STESTSSESSION0000000000000");
    let image = rumu_core::Attachment {
        kind: rumu_core::AttachmentKind::Image,
        hash: "0".repeat(64),
        mime: "image/png".into(),
        name: Some("shot.png".into()),
        width: 800,
        height: 600,
        size: 10,
    };
    let mk = |id: &str, body: NodeBody| Node {
        id: rumu_core::NodeId::from(id),
        parent: None,
        session: sid.clone(),
        at: 0,
        dur_ms: None,
        body,
    };
    let nodes = vec![
        mk(
            "U1",
            NodeBody::User {
                content: rumu_core::Content::text("look at this [Image #1]")
                    .with_attachments([image.clone()]),
            },
        ),
        mk(
            "T1",
            NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text("image proj:a.png (800×600, 9 bytes stored)")
                    .with_attachments([image]),
                is_error: false,
                invoke: None,
            },
        ),
    ];
    let info = rumu_core::SessionInfo {
        id: sid.clone(),
        title: "with images".into(),
        active_branch: rumu_core::BranchId::from("B1"),
        created_at: 0,
        updated_at: 0,
        touched_at: 0,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    let host = session_host(info, nodes, vec![]);
    let cx = host.cx();
    let plugin = session_plugin();
    let base = "@rumu:/sessions/01STESTSSESSION0000000000000";

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .contains("look at this [image: shot.png 800×600]"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("[Image #1]"),
        "{}",
        out.content.text
    );

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/T1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .contains("image proj:a.png (800×600, 9 bytes stored)\n[image: shot.png 800×600]"),
        "{}",
        out.content.text
    );
}

/// Range reads keep the head and tail of a long node body around a
/// `<truncated>` pointer; single-node reads stay whole.
#[tokio::test]
async fn session_range_truncation() {
    let (mut info, mut nodes, branches) = staged_session();
    let head = "a".repeat(1000);
    let tail = "b".repeat(1000);
    let long = format!("{head}{}{tail}", "~".repeat(500));
    let idx = nodes.iter().position(|n| n.id.0 == "U1").unwrap();
    nodes[idx] = Node {
        at: 2000,
        ..nodes[idx].clone()
    };
    if let NodeBody::User { content } = &mut nodes[idx].body {
        content.text = long.clone();
    }
    info.title = "long".into();
    let host = session_host(info, nodes, branches);
    let cx = host.cx();
    let plugin = session_plugin();
    let base = "@rumu:/sessions/01STESTSSESSION0000000000000";

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1..T1") }), &cx)
        .await
        .unwrap();
    let pointer =
        "<truncated chars=\"500\" read=\"@rumu:/sessions/01STESTSSESSION0000000000000/U1\"/>";
    assert!(
        out.content.text.contains(pointer),
        "{}…",
        &out.content.text[..200.min(out.content.text.len())]
    );
    assert!(
        out.content
            .text
            .contains(&format!("{head}\n{pointer}\n{tail}")),
        "head and tail kept in order"
    );
    assert!(!out.content.text.contains('~'), "the middle is dropped");
    assert!(
        out.content.text.chars().count() < 2000 + "~".repeat(500).len(),
        "capped in ranges"
    );

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1") }), &cx)
        .await
        .unwrap();
    // open tag + newline + body + newline + close tag + trailing newline
    let open = "<node id=\"U1\" kind=\"user\">\n";
    assert_eq!(
        out.content.text.chars().count(),
        open.len() + 2500 + "\n</node>\n".len(),
        "single-node read is whole"
    );
    assert!(
        out.content.text.contains(&long),
        "single-node read is whole"
    );
}

/// A node of a staged session tree.
fn sess_node(sid: &str, id: &str, parent: Option<&str>, at: i64, body: NodeBody) -> Node {
    Node {
        id: rumu_core::NodeId::from(id),
        parent: parent.map(rumu_core::NodeId::from),
        session: rumu_core::SessionId::from(sid),
        at,
        dur_ms: None,
        body,
    }
}

/// Stage one main branch ending at `head`.
fn staged_tree(sid: &str, nodes: Vec<Node>, head: &str) -> std::sync::Arc<FakeRuntime> {
    let info = rumu_core::SessionInfo {
        id: rumu_core::SessionId::from(sid),
        title: "staged".into(),
        active_branch: rumu_core::BranchId::from("B"),
        created_at: 1,
        updated_at: nodes.iter().map(|n| n.at).max().unwrap_or(0),
        touched_at: 0,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    let branch = rumu_core::Branch {
        id: rumu_core::BranchId::from("B"),
        session: rumu_core::SessionId::from(sid),
        head: rumu_core::NodeId::from(head),
        forked_from: None,
        kind: rumu_core::BranchKind::Main,
        status: rumu_core::BranchStatus::Active,
        created_at: 1,
    };
    session_host(info, nodes, vec![branch])
}

/// Body text that looks like our markup is escaped; markdown-ish `<x>` that
/// cannot forge an element is left alone, and our own tags are not escaped.
#[tokio::test]
async fn session_render_escapes_markup() {
    let sid = "01SESCAPE000000000000000000";
    let nodes = vec![
        sess_node(
            sid,
            "R",
            None,
            1,
            NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
            },
        ),
        sess_node(
            sid,
            "U1",
            Some("R"),
            2,
            NodeBody::User {
                content: rumu_core::Content::text(
                    "read <node id=\"x\">, </node>, <call> and <b> stays",
                ),
            },
        ),
        sess_node(
            sid,
            "A1",
            Some("U1"),
            3,
            NodeBody::Assistant {
                content: rumu_core::Content::text("echo <node>"),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall::new(
                    "c1",
                    "bash",
                    json!({ "command": "printf '<elided>'" }),
                )],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
        sess_node(
            sid,
            "T1",
            Some("A1"),
            4,
            NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text("<truncated and <plain>"),
                is_error: false,
                invoke: None,
            },
        ),
        sess_node(
            sid,
            "N1",
            Some("T1"),
            5,
            NodeBody::Note {
                source: "memory".into(),
                content: rumu_core::Content {
                    text: "note".into(),
                    attachments: Vec::new(),
                    data: Some(Tagged {
                        tag: "memo".into(),
                        v: 1,
                        data: json!({ "k": 1 }),
                    }),
                },
            },
        ),
    ];
    let host = staged_tree(sid, nodes, "N1");
    let cx = host.cx();
    let plugin = session_plugin();

    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/sessions/{sid}/R..N1") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = &out.content.text;
    assert!(
        text.contains("read &lt;node id=\"x\">, &lt;/node>, &lt;call> and <b> stays"),
        "{text}"
    );
    assert!(text.contains("echo &lt;node>"), "{text}");
    assert!(
        text.contains(
            "<call id=\"c1\" name=\"bash\">{\"command\":\"printf '&lt;elided>'\"}</call>"
        ),
        "{text}"
    );
    assert!(
        text.contains("&lt;truncated and <plain>"),
        "tool-result text: {text}"
    );
    assert!(
        text.contains("<node id=\"N1\" kind=\"note\" source=\"memory\" tag=\"memo\">"),
        "note tag attr: {text}"
    );
    // Our generated tags survive unescaped.
    assert!(
        text.contains("<node id=\"T1\" kind=\"tool_result\" call=\"c1\" tool=\"bash\">"),
        "{text}"
    );
}

/// A nested session read is folded to `<elided>` in range reads only; the
/// tool name is found through a parallel-results chain, not just the parent.
#[tokio::test]
async fn session_range_folds_nested_session_reads() {
    let sid = "01SFOLD00000000000000000000";
    let nested = "very long nested transcript";
    let nodes = vec![
        sess_node(
            sid,
            "U1",
            None,
            1,
            NodeBody::User {
                content: rumu_core::Content::text("go"),
            },
        ),
        sess_node(
            sid,
            "A1",
            Some("U1"),
            2,
            NodeBody::Assistant {
                content: rumu_core::Content::text(""),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![
                    rumu_core::ToolCall::new(
                        "c1",
                        "read",
                        json!({ "path": "@rumu:/sessions/other/abc" }),
                    ),
                    rumu_core::ToolCall::new(
                        "c2",
                        "search",
                        json!({ "query": "x", "scope": "@rumu:/sessions/other" }),
                    ),
                    rumu_core::ToolCall::new("c3", "bash", json!({ "command": "ls" })),
                ],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
        sess_node(
            sid,
            "T1",
            Some("A1"),
            3,
            NodeBody::ToolResult {
                call_id: "c1".into(),
                content: rumu_core::Content::text(nested),
                is_error: false,
                invoke: None,
            },
        ),
        sess_node(
            sid,
            "T2",
            Some("T1"),
            4,
            NodeBody::ToolResult {
                call_id: "c2".into(),
                content: rumu_core::Content::text("search results"),
                is_error: false,
                invoke: None,
            },
        ),
        sess_node(
            sid,
            "T3",
            Some("T2"),
            5,
            NodeBody::ToolResult {
                call_id: "c3".into(),
                content: rumu_core::Content::text("file-a"),
                is_error: true,
                invoke: None,
            },
        ),
    ];
    let host = staged_tree(sid, nodes, "T3");
    let cx = host.cx();
    let plugin = session_plugin();
    let base = format!("@rumu:/sessions/{sid}");

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1..T3") }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = &out.content.text;
    // T1 and T2 (read, search) are folded, with tool found up the chain.
    assert!(
        text.contains(&*format!(
            "<node id=\"T1\" kind=\"tool_result\" call=\"c1\" tool=\"read\">\n\
         <elided chars=\"{}\" read=\"{base}/T1\"/>",
            nested.chars().count()
        )),
        "{text}"
    );
    assert!(
        text.contains(
            "<node id=\"T2\" kind=\"tool_result\" call=\"c2\" tool=\"search\">\n\
             <elided chars=\"14\" read=\""
        ),
        "{text}"
    );
    assert!(!text.contains(nested), "folded body is gone: {text}");
    // T3 (bash) is not folded, carries `error`, and its tool is resolved
    // through T2→T1→A1.
    assert!(
        text.contains(
            "<node id=\"T3\" kind=\"tool_result\" call=\"c3\" tool=\"bash\" error=\"true\">\nfile-a"
        ),
        "{text}"
    );

    // A single-node read is never folded.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/T1") }), &cx)
        .await
        .unwrap();
    assert!(out.content.text.contains(nested), "{}", out.content.text);
    assert!(
        !out.content.text.contains("<elided"),
        "{}",
        out.content.text
    );
}

/// Range reads clip long tool-call args and mark the dropped count; a
/// single-node read keeps them whole.
#[tokio::test]
async fn session_call_args_clip_only_in_ranges() {
    let sid = "01SCLIP00000000000000000000";
    let command = "x".repeat(400);
    let dropped = json!({ "command": command.clone() })
        .to_string()
        .chars()
        .count()
        - 300;
    let nodes = vec![
        sess_node(
            sid,
            "U1",
            None,
            1,
            NodeBody::User {
                content: rumu_core::Content::text("go"),
            },
        ),
        sess_node(
            sid,
            "A1",
            Some("U1"),
            2,
            NodeBody::Assistant {
                content: rumu_core::Content::text("run"),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall::new(
                    "c1",
                    "bash",
                    json!({ "command": command }),
                )],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
    ];
    let host = staged_tree(sid, nodes, "A1");
    let cx = host.cx();
    let plugin = session_plugin();
    let base = format!("@rumu:/sessions/{sid}");

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/U1..A1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains(&format!(
            "<call id=\"c1\" name=\"bash\" truncated=\"{dropped}\">"
        )),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains(&command),
        "args are clipped in ranges"
    );

    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/A1") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains(&command),
        "single-node reads keep args whole"
    );
    assert!(
        !out.content.text.contains("truncated="),
        "{}",
        out.content.text
    );
}

/// A long assistant body truncates only its prose: every `<call>` line stays
/// whole, so the range output never contains a split tag.
#[tokio::test]
async fn session_range_truncation_keeps_call_tags_whole() {
    let sid = "01SCALLS0000000000000000000";
    let calls: Vec<rumu_core::ToolCall> = (0..10)
        .map(|i| {
            rumu_core::ToolCall::new(
                format!("c{i}"),
                "bash",
                json!({ "command": "y".repeat(280) }),
            )
        })
        .collect();
    let nodes = vec![
        sess_node(
            sid,
            "U1",
            None,
            1,
            NodeBody::User {
                content: rumu_core::Content::text("go"),
            },
        ),
        sess_node(
            sid,
            "A1",
            Some("U1"),
            2,
            NodeBody::Assistant {
                content: rumu_core::Content::text("z".repeat(2500)),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: calls,
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
    ];
    let host = staged_tree(sid, nodes, "A1");
    let cx = host.cx();
    let plugin = session_plugin();

    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/sessions/{sid}/U1..A1") }),
            &cx,
        )
        .await
        .unwrap();
    let text = &out.content.text;
    assert!(
        text.contains(&format!(
            "<truncated chars=\"500\" read=\"@rumu:/sessions/{sid}/A1\"/>"
        )),
        "{text}"
    );
    for i in 0..10 {
        assert!(
            text.contains(&format!("<call id=\"c{i}\" name=\"bash\">")),
            "call c{i} intact: {text}"
        );
    }
    assert_eq!(
        text.matches("<call ").count(),
        text.matches("</call>").count()
    );
}

/// Node addresses resolve by full id or unique prefix; prefix collisions
/// lengthen the session's abbreviation.
#[tokio::test]
async fn session_prefix_resolution_and_abbreviations() {
    let sid = "01SPREFIX000000000000000000";
    let root = "abcdefgh1-0000-4000-8000-000000000001";
    let user = "abcdefgh2-0000-4000-8000-000000000002";
    let reply = "zzzzzzzz9-0000-4000-8000-000000000003";
    let nodes = vec![
        sess_node(
            sid,
            root,
            None,
            1,
            NodeBody::Note {
                source: "system".into(),
                content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
            },
        ),
        sess_node(
            sid,
            user,
            Some(root),
            2,
            NodeBody::User {
                content: rumu_core::Content::text("hi"),
            },
        ),
        sess_node(
            sid,
            reply,
            Some(user),
            3,
            NodeBody::Assistant {
                content: rumu_core::Content::text("done"),
                reasoning: vec![],
                stats: Default::default(),
                reasoning_reset: false,
                tool_calls: vec![],
                model: "fake".into(),
                usage: Default::default(),
            },
        ),
    ];
    let host = staged_tree(sid, nodes, reply);
    let cx = host.cx();
    let plugin = session_plugin();
    let base = format!("@rumu:/sessions/{sid}");

    // A unique 4-char prefix resolves.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/zzzz") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .starts_with("<node id=\"zzzzzzzz9\" kind=\"assistant\">\n"),
        "{}",
        out.content.text
    );

    // The full id resolves even though a shorter prefix would not.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/{user}") }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("kind=\"user\""),
        "{}",
        out.content.text
    );

    // `abcdefgh` is shared by two nodes: ambiguous.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/abcdefgh") }), &cx)
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains(&format!(
            "node prefix `abcdefgh` is ambiguous on session `{sid}`: {root}, {user}"
        )),
        "{}",
        out.content.text
    );

    // Too short to resolve, and not an exact id.
    let out = plugin
        .call_tool("read", json!({ "path": format!("{base}/zz") }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no node `zz`"),
        "{}",
        out.content.text
    );

    // The abbreviations grew to 9 chars so the shared 8-char prefix is
    // distinct; range endpoints work as prefixes too.
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("{base}/abcdefgh1..zzzzzzzz9") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("<node id=\"abcdefgh1\""),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("<node id=\"abcdefgh2\""),
        "{}",
        out.content.text
    );
}

// ---------- search ----------
/// Two sessions with distinct node text; the older one updated first.
/// Full node addresses of a session-search output, in output order: each
/// indented node line joined to the group header above it.
fn hit_addrs(text: &str) -> Vec<String> {
    let mut session = "";
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with("@rumu:/sessions/") {
            session = line.split("  ").next().unwrap();
        } else if let Some(rest) = line.strip_prefix("  ") {
            out.push(format!("{session}/{}", rest.split("  ").next().unwrap()));
        }
    }
    out
}

fn search_sessions() -> std::sync::Arc<FakeRuntime> {
    let mk = |sid: &str, nid: &str, parent: Option<&str>, at: i64, body: NodeBody| Node {
        id: rumu_core::NodeId::from(nid),
        parent: parent.map(rumu_core::NodeId::from),
        session: rumu_core::SessionId::from(sid),
        at,
        dur_ms: None,
        body,
    };
    let info = |sid: &str, title: &str, updated: i64| rumu_core::SessionInfo {
        id: rumu_core::SessionId::from(sid),
        title: title.into(),
        active_branch: rumu_core::BranchId::from("B"),
        created_at: 1,
        updated_at: updated,
        touched_at: updated,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    let branch = |sid: &str, head: &str| rumu_core::Branch {
        id: rumu_core::BranchId::from("B"),
        session: rumu_core::SessionId::from(sid),
        head: rumu_core::NodeId::from(head),
        forked_from: None,
        kind: rumu_core::BranchKind::Main,
        status: rumu_core::BranchStatus::Active,
        created_at: 1,
    };
    let host = FakeRuntime::arc();
    // s1 (newer): a user node about ssh config, an assistant node with a
    // tool call, and an unrelated node.
    let s1 = "01SSESSIONONE000000000000000";
    let s2 = "01SSESSIONTWO000000000000000";
    host.sessions
        .lock()
        .unwrap()
        .extend([info(s2, "older chat", 1000), info(s1, "ssh setup", 9000)]);
    host.session_trees.lock().unwrap().extend([
        (
            rumu_core::SessionId::from(s2),
            info(s2, "older chat", 1000),
            vec![
                mk(
                    s2,
                    "R2",
                    None,
                    100,
                    NodeBody::Note {
                        source: "system".into(),
                        content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
                    },
                ),
                mk(
                    s2,
                    "U2",
                    Some("R2"),
                    200,
                    NodeBody::User {
                        content: rumu_core::Content::text("fix the ssh config on the server"),
                    },
                ),
            ],
            vec![branch(s2, "U2")],
        ),
        (
            rumu_core::SessionId::from(s1),
            info(s1, "ssh setup", 9000),
            vec![
                mk(
                    s1,
                    "R1",
                    None,
                    100,
                    NodeBody::Note {
                        source: "system".into(),
                        content: rumu_core::Content::marker(&rumu_core::SessionRoot::default()),
                    },
                ),
                mk(
                    s1,
                    "U1",
                    Some("R1"),
                    200,
                    NodeBody::User {
                        content: rumu_core::Content::text("how do I rotate the ssh keys"),
                    },
                ),
                mk(
                    s1,
                    "A1",
                    Some("U1"),
                    300,
                    NodeBody::Assistant {
                        content: rumu_core::Content::text("checking the config"),
                        reasoning: vec![],
                        stats: Default::default(),
                        reasoning_reset: false,
                        tool_calls: vec![rumu_core::ToolCall::new(
                            "c1",
                            "bash",
                            json!({ "command": "cat ~/.ssh/config" }),
                        )],
                        model: "fake".into(),
                        usage: Default::default(),
                    },
                ),
                mk(
                    s1,
                    "T1",
                    Some("A1"),
                    400,
                    NodeBody::ToolResult {
                        call_id: "c1".into(),
                        content: rumu_core::Content::text(
                            "Host server\n  Port 22\n  IdentityFile ~/.ssh/id_ed25519",
                        ),
                        is_error: false,
                        invoke: None,
                    },
                ),
                mk(
                    s1,
                    "N1",
                    Some("T1"),
                    500,
                    NodeBody::Note {
                        source: "memory".into(),
                        content: rumu_core::Content::text("ssh-agent notes"),
                    },
                ),
            ],
            vec![branch(s1, "N1")],
        ),
    ]);
    host
}

#[tokio::test]
async fn search_sessions_and_cards_by_scope() {
    let host = search_sessions();
    let cx = host.cx();
    let s1 = "01SSESSIONONE000000000000000";
    let s2 = "01SSESSIONTWO000000000000000";
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::put_card(
        home.path(),
        "main",
        "gateway",
        "---\ntitle: Gateway\n---\nthe example server runs nginx and sshd\n",
    );
    let plugin = deck_plugin(home.path());

    // Default scope = cards; hits are `@rumu:/cards/self/<id>` addresses.
    let out = plugin
        .call_tool("search", json!({ "query": "gateway" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("@rumu:/cards/self/gateway"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("@rumu:/sessions/"),
        "card scope by default: {}",
        out.content.text
    );

    // Session scope: BM25-ranked matches across node kinds (user text,
    // assistant text + tool args, tool-result text, notes); equal scores are
    // newest first by node time. The tool result is downweighted.
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "SSH", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    // Hits group by session (best hit first), ranked within a group.
    assert_eq!(
        hit_addrs(&out.content.text),
        [
            format!("@rumu:/sessions/{s1}/N1"),
            format!("@rumu:/sessions/{s1}/U1"),
            format!("@rumu:/sessions/{s1}/A1"),
            format!("@rumu:/sessions/{s1}/T1"),
            format!("@rumu:/sessions/{s2}/U2"),
        ],
        "{}",
        out.content.text
    );
    // One header per session with its quoted title; node lines carry kind,
    // time and a quoted snippet.
    assert!(
        out.content
            .text
            .starts_with(&format!("@rumu:/sessions/{s1}  \"ssh setup\"\n  N1  note ")),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("  \"ssh-agent notes\""),
        "{}",
        out.content.text
    );

    // Terms are ORed: "ssh rotate" puts U1 (both terms) first, and nodes
    // matching only "ssh" still appear below.
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "ssh rotate", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    let addrs = hit_addrs(&out.content.text);
    assert_eq!(
        addrs.first(),
        Some(&format!("@rumu:/sessions/{s1}/U1")),
        "{}",
        out.content.text
    );
    assert!(
        addrs.contains(&format!("@rumu:/sessions/{s2}/U2")),
        "{}",
        out.content.text
    );
    // Tool-call args are searchable ("~/.ssh/config" is in A1).
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": ".ssh/config", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        hit_addrs(&out.content.text).contains(&format!("@rumu:/sessions/{s1}/A1")),
        "{}",
        out.content.text
    );

    // One session only.
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "ssh", "scope": format!("@rumu:/sessions/{s2}") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        hit_addrs(&out.content.text) == [format!("@rumu:/sessions/{s2}/U2")],
        "{}",
        out.content.text
    );

    // limit + the more-hits footnote.
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "ssh", "scope": "@rumu:/sessions", "limit": 2 }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.content.text.contains("(+3 more hits)"),
        "{}",
        out.content.text
    );

    // A miss reports no hits, not an error.
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "no-such-word", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        !out.is_error && out.content.text.contains("no conversation nodes match"),
        "{}",
        out.content.text
    );
    assert!(
        out.effects.is_empty(),
        "searching records no card_use markers"
    );

    // Scopes outside the @rumu workspace are rejected.
    for scope in ["proj:src", "/etc", "@rumu:/sessionsx"] {
        let out = plugin
            .call_tool("search", json!({ "query": "x", "scope": scope }), &cx)
            .await
            .unwrap();
        assert!(out.is_error, "{scope}: {}", out.content.text);
        assert!(
            out.content.text.contains("`scope` must address"),
            "{scope}: {}",
            out.content.text
        );
    }
}

/// A spaceless CJK query still prints snippets (snippet terms come
/// from the ranker's tokenizer, not `split_whitespace`), and a one-char
/// query hits: explicit search indexes a run's chars.
#[tokio::test]
async fn search_sessions_snippet_cjk_queries() {
    let host = FakeRuntime::arc();
    let sid = rumu_core::SessionId::from("01SSESSIONCJK000000000000000");
    let info = rumu_core::SessionInfo {
        id: sid.clone(),
        title: "服务器".into(),
        active_branch: rumu_core::BranchId::from("B"),
        created_at: 1,
        updated_at: 1,
        touched_at: 1,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    let mk = |nid: &str, at: i64, body: NodeBody| Node {
        id: rumu_core::NodeId::from(nid),
        parent: None,
        session: sid.clone(),
        at,
        dur_ms: None,
        body,
    };
    host.sessions.lock().unwrap().push(info.clone());
    host.session_trees.lock().unwrap().push((
        sid.clone(),
        info,
        vec![mk(
            "U1",
            100,
            NodeBody::User {
                content: rumu_core::Content::text("帮我配置代理服务器"),
            },
        )],
        vec![],
    ));
    let home = tempfile::tempdir().unwrap();
    let plugin = deck_plugin(home.path());
    let cx = host.cx();

    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "服务器", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        hit_addrs(&out.content.text).contains(&format!("@rumu:/sessions/{sid}/U1")),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("  \"帮我配置代理服务器\""),
        "a spaceless CJK query still prints a snippet: {}",
        out.content.text
    );

    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "服", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        hit_addrs(&out.content.text).contains(&format!("@rumu:/sessions/{sid}/U1")),
        "{}",
        out.content.text
    );
}

/// Equal text scores equally; a tool result is halved, and a score tie is
/// broken newest first by node time.
#[tokio::test]
async fn search_downweights_tool_results_and_breaks_ties_by_time() {
    let host = FakeRuntime::arc();
    let sid = rumu_core::SessionId::from("01SSESSIONTOOLS00000000000000");
    let info = rumu_core::SessionInfo {
        id: sid.clone(),
        title: "line one \"quoted\"\nline two, long enough to be cut off here".into(),
        active_branch: rumu_core::BranchId::from("B"),
        created_at: 1,
        updated_at: 1,
        touched_at: 1,
        seen_at: None,
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        project: None,
    };
    let mk = |nid: &str, at: i64, body: NodeBody| Node {
        id: rumu_core::NodeId::from(nid),
        parent: None,
        session: sid.clone(),
        at,
        dur_ms: None,
        body,
    };
    host.sessions.lock().unwrap().push(info.clone());
    host.session_trees.lock().unwrap().push((
        sid.clone(),
        info,
        vec![
            mk(
                "UOLD",
                100,
                NodeBody::User {
                    content: rumu_core::Content::text("widget"),
                },
            ),
            mk(
                "UNEW",
                200,
                NodeBody::User {
                    content: rumu_core::Content::text("widget"),
                },
            ),
            mk(
                "LOOK",
                250,
                NodeBody::Assistant {
                    content: rumu_core::Content::text(""),
                    reasoning: vec![],
                    stats: Default::default(),
                    reasoning_reset: false,
                    tool_calls: vec![rumu_core::ToolCall::new(
                        "c0",
                        "search",
                        json!({ "query": "widget" }),
                    )],
                    model: "fake".into(),
                    usage: Default::default(),
                },
            ),
            mk(
                "TOOL",
                300,
                NodeBody::ToolResult {
                    call_id: "c1".into(),
                    content: rumu_core::Content::text("widget"),
                    is_error: false,
                    invoke: None,
                },
            ),
        ],
        vec![],
    ));
    let home = tempfile::tempdir().unwrap();
    let plugin = deck_plugin(home.path());
    let cx = host.cx();
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "widget", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    let text = &out.content.text;
    assert_eq!(
        hit_addrs(text),
        [
            format!("@rumu:/sessions/{sid}/UNEW"),
            format!("@rumu:/sessions/{sid}/UOLD"),
            format!("@rumu:/sessions/{sid}/TOOL"),
        ],
        "lookup-tool args are not indexed: {text}"
    );
    // The title is a JSON literal cut to 48 chars, `…` outside the quotes.
    assert!(
        text.starts_with(&format!(
            "@rumu:/sessions/{sid}  \"line one \\\"quoted\\\"\\nline two, long enough to be cu\"…\n"
        )),
        "{text}"
    );
    // A repeated snippet points back at its first node.
    assert!(text.contains("  \"widget\""), "{text}");
    assert_eq!(text.matches("(same as UNEW)").count(), 2, "{text}");
}

// ---------- bash progress + background ----------

/// `bash` streams output chunks to `cx.progress` as they arrive.
#[tokio::test]
async fn bash_streams_progress_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, host, cx, _home) = setup(dir.path(), false);
    let out = plugin
        .call_tool(
            "bash",
            json!({ "command": "printf 'one\\n'; sleep 0.05; printf 'two\\n'" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let chunks: String = host
        .progress
        .lock()
        .unwrap()
        .iter()
        .filter_map(|(_, _, p)| match p {
            rumu_core::Progress::Append(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(chunks.contains("one"), "chunks: {chunks:?}");
    assert!(chunks.contains("two"), "chunks: {chunks:?}");
}

/// `tail` keeps the last lines for the model, but streams all of them and
/// keeps the command's own exit code.
#[tokio::test]
async fn bash_tail_keeps_the_last_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, host, cx, _home) = setup(dir.path(), false);
    let run = |tail: usize| {
        plugin.call_tool(
            "bash",
            json!({ "command": "seq 1 5; exit 3", "tail": tail }),
            &cx,
        )
    };
    let out = run(2).await.unwrap();
    assert!(out.is_error);
    assert_eq!(
        out.content.text,
        "exit code: 3\n… [3 earlier lines omitted] …\n4\n5\n"
    );
    let streamed = host
        .progress
        .lock()
        .unwrap()
        .iter()
        .any(|(_, _, p)| matches!(p, rumu_core::Progress::Append(s) if s.starts_with("1\n")));
    assert!(streamed);
    let out = run(5).await.unwrap();
    assert_eq!(out.content.text, "exit code: 3\n1\n2\n3\n4\n5\n");
}

/// A background bash future that is dropped kills its process tree: the
/// shell and the processes it started.
#[tokio::test]
async fn background_bash_is_killed_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let (plugin, host, _cx, _home) = setup(dir.path(), false);
    let cx = host.cx_with(true);
    let script = format!("sleep 60 & echo $$ $! > {}; wait", pid_file.display());
    let args = json!({ "command": script });

    let handle = tokio::spawn(async move {
        let _ = plugin.call_tool("bash", args, &cx).await;
    });
    // Wait until the shell wrote its pids, then drop the future.
    let mut pids = None;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Some((shell, child)) = text.trim().split_once(' ')
            && let (Ok(shell), Ok(child)) = (shell.parse::<i32>(), child.parse::<i32>())
        {
            pids = Some([shell, child]);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pids = pids.expect("bash started");
    handle.abort();
    let _ = handle.await;

    for pid in pids {
        let mut gone = false;
        for _ in 0..200 {
            let alive = std::process::Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !alive {
                gone = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(gone, "process {pid} survived the dropped bash future");
    }
}

/// A shell that exits on its own returns at once and leaves the processes it
/// detached (`… > /dev/null &`) running.
#[tokio::test]
async fn bash_leaves_detached_processes_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);
    let args = json!({ "command": "sleep 30 > /dev/null 2>&1 & echo $!", "timeout": 5 });
    let out = plugin.call_tool("bash", args, &cx).await.unwrap();
    let text = out.content.text.clone();
    let pid: i32 = text
        .lines()
        .last()
        .and_then(|l| l.trim().parse().ok())
        .filter(|&p: &i32| p > 1)
        .unwrap_or_else(|| panic!("no pid in {text:?}"));
    let alive = |pid: i32| {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    };
    assert!(alive(pid), "detached process {pid} was killed");
    let _ = std::process::Command::new("kill")
        .arg(pid.to_string())
        .status();
}

/// `read @rumu:/tasks/<id>` renders the task's status and buffered output.
#[tokio::test]
async fn task_read_renders_status_and_output() {
    use rumu_core::{Content, TaskId, TaskInfo, TaskKind, TaskStatus};
    let home = tempfile::tempdir().unwrap();
    let plugin = deck_plugin(home.path());
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let info = TaskInfo {
        id: TaskId::from("t1"),
        session: cx.session.clone(),
        branch: cx.branch.clone(),
        call_id: "c1".into(),
        kind: TaskKind::Tool {
            name: "bash".into(),
            args_summary: "ls".into(),
        },
        summary: "ls".into(),
        status: TaskStatus::Done,
        started_at: 1,
        ended_at: Some(2),
        truncated: false,
        is_error: false,
        result: Some(Content::text("ignored")),
        data: None,
    };
    host.tasks
        .lock()
        .unwrap()
        .push((info, "partial\nmore output\n".into()));

    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/tasks/t1" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("task t1 · ok · bash: ls"));
    assert!(out.content.text.contains("partial\nmore output"));
    assert!(out.content.text.contains("--- result ---\nignored"));

    let listing = plugin
        .call_tool("read", json!({ "path": "@rumu:/tasks" }), &cx)
        .await
        .unwrap();
    assert!(!listing.is_error, "{}", listing.content.text);
    assert!(listing.content.text.contains("@rumu:/tasks/t1"));
}

// ---------- decks, mounts and history ----------

/// The CAS refusal names the changes since the seen version and shows the diff.
#[tokio::test]
async fn cas_refusal_lists_changes_and_diff() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "a", "one\n");
    let (plugin, host, cx) = card_plugin_for(home.path());
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/a" }), &cx)
        .await
        .unwrap();
    assert!(!read.is_error, "{}", read.content.text);
    flush_output(&host, "call-1", &read);

    // Another assistant changes the card through the deck history.
    let decks = plugin.decks();
    let deck = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap();
    let cards = rumu_cards::Cards::new(
        decks,
        &[rumu_core::CardMount {
            deck: deck.id().to_owned(),
            at: "/cards/self".into(),
        }],
    );
    cards
        .write(
            "self/a",
            "---\ntitle: other\n---\ntwo\n",
            None,
            None,
            &rumu_cards::deck::Author {
                id: "other-id".into(),
                name: "other-assistant".into(),
                session: Some("s2".into()),
                node: Some("n2".into()),
            },
        )
        .unwrap();

    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/a", "content": "three\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("changed since you last read it"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("by other-assistant"),
        "{}",
        out.content.text
    );
    assert!(out.content.text.contains("Diff"), "{}", out.content.text);
    assert!(out.content.text.contains("-one"), "{}", out.content.text);
    assert!(out.content.text.contains("+two"), "{}", out.content.text);
}

/// Mounts beyond the grant ask the user; an always-allow grant removes the
/// prompt, and overlapping mount points are refused.
#[tokio::test]
async fn mounts_need_a_grant_and_must_not_overlap() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "extra", None, &[]);
    crate::testutil::put_card(home.path(), "extra", "e", "extra body\n");
    let (plugin, _host, _cx) = card_plugin_for(home.path());
    let decks = plugin.decks();
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let extra = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "extra")
        .unwrap()
        .id()
        .to_owned();
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = PathState {
        workspaces: vec![],
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: main.clone(),
        mounts: vec![rumu_core::CardMount {
            deck: main.clone(),
            at: "/cards/self".into(),
        }],
        ..Default::default()
    };
    let cx = host.cx();

    // No grant: mounting goes through the user, and this host cannot ask.
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": extra, "at": "/cards/extra" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("this host cannot ask"),
        "{}",
        out.content.text
    );

    // An explicit grant lets it mount; the note carries the marker.
    decks.remember(
        rumu_cards::deck::Action::Mount,
        rumu_cards::deck::Scope::Always,
        "",
        &main,
        &extra,
    );
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": extra, "at": "/cards/extra" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content
            .text
            .contains("**Card catalog** (`@rumu:/cards/extra`)"),
        "the mount result carries the deck's catalog: {}",
        out.content.text
    );
    assert!(
        out.content.text.contains("[extra/e](@rumu:/cards/extra/e)"),
        "{}",
        out.content.text
    );
    flush_output(&host, "call-1", &out);
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: extra.clone(),
            at: "/cards/extra".into(),
        });

    // A mount point may not overlap an existing one.
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": extra, "at": "/cards/extra/deep" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("overlaps"),
        "{}",
        out.content.text
    );

    // The mounted deck now feeds listing.
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/cards" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("@rumu:/cards/extra/e"),
        "{}",
        out.content.text
    );

    // Unmounting says the content is no longer available.
    let out = plugin
        .call_tool("card_mount", json!({ "at": "/cards/extra" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("unmounted"),
        "{}",
        out.content.text
    );
}

/// Links resolve one level and never accept writes.
#[tokio::test]
async fn links_read_their_target_and_refuse_writes() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "other", None, &[]);
    crate::testutil::put_card(home.path(), "other", "t", "target body\n");
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let decks = plugin.decks();
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let other = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "other")
        .unwrap()
        .id()
        .to_owned();
    decks.remember(
        rumu_cards::deck::Action::Link,
        rumu_cards::deck::Scope::Always,
        "",
        &main,
        &other,
    );
    *host.state.lock().unwrap() = PathState {
        workspaces: vec![],
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: main.clone(),
        mounts: vec![
            rumu_core::CardMount {
                deck: main.clone(),
                at: "/cards/self".into(),
            },
            rumu_core::CardMount {
                deck: other.clone(),
                at: "/cards/other".into(),
            },
        ],
        ..Default::default()
    };
    let cx = host.cx();

    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/link",
                "content": "---\nlink: @rumu:/cards/other/t\n---\nignored\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    flush_output(&host, "call-1", &out);

    // Reading the link shows the target, under the target's location.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/link" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("target body"),
        "{}",
        out.content.text
    );
    let header: card::CardReadHeader = out.content.decode().expect("read header");
    assert_eq!(header.deck, other);
    assert_eq!(header.path, "t");

    // Writes through the link are refused.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/link", "content": "x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("is a link"),
        "{}",
        out.content.text
    );
}

/// The `@rumu:/history` timeline, per-card versions and one version's diff.
#[tokio::test]
async fn history_listing_and_version_reads() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "a", "one\n");
    let (plugin, host, cx) = card_plugin_for(home.path());
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/a" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    let write = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/a", "content": "two\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!write.is_error, "{}", write.content.text);
    flush_output(&host, "call-1", &write);

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/history" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("self/a"), "{}", out.content.text);
    assert!(out.content.text.contains("update"), "{}", out.content.text);

    let out = plugin
        .call_tool(
            "list",
            json!({ "path": "@rumu:/history/cards/self/a" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("update"), "{}", out.content.text);

    // Read the latest version: a diff from the previous one.
    let commit = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .commits(1)
        .unwrap()
        .remove(0);
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/history/cards/self/a@{}", commit.short) }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("-one"), "{}", out.content.text);
    assert!(out.content.text.contains("+two"), "{}", out.content.text);
}

/// A deleted card's history stays reachable at its old path.
#[tokio::test]
async fn deleted_card_history_stays_at_its_old_path() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "gone", "old body\n");
    let (plugin, _host, cx) = card_plugin_for(home.path());
    crate::testutil::remove_card(home.path(), "main", "gone");

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/history" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("self/gone  delete"),
        "{}",
        out.content.text
    );

    let out = plugin
        .call_tool(
            "list",
            json!({ "path": "@rumu:/history/cards/self/gone" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("delete"), "{}", out.content.text);
    let first = out
        .content
        .text
        .lines()
        .last()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();

    // The old version is readable through the same path.
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/history/cards/self/gone@{first}") }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("old body"),
        "{}",
        out.content.text
    );

    // A commit that does not touch the card is refused.
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": "@rumu:/history/cards/self/gone@0000000" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
}

/// Writing to another deck asks the user, deck by deck; "allow for this
/// session" covers later writes of the session, and a decline writes nothing.
#[tokio::test]
async fn writes_to_other_decks_need_approval() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "user", None, &[]);
    crate::testutil::put_card(home.path(), "main", "tz", "UTC+8\n");
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let user = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "user")
        .unwrap()
        .id()
        .to_owned();
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: user.clone(),
            at: "/cards/user".into(),
        });
    let cx = host.cx();
    let write = |path: &str, from: Option<&str>| {
        let mut args = json!({ "path": path, "content": "UTC+8\n" });
        if let Some(from) = from {
            args["from"] = json!(from);
        }
        args
    };

    // The own deck never asks.
    let out = plugin
        .call_tool("write", write("@rumu:/cards/self/a", None), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(host.decisions.lock().unwrap().is_empty());

    // A declined write to `user` leaves nothing behind.
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": false }));
    let out = plugin
        .call_tool("write", write("@rumu:/cards/user/a", None), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("declined"),
        "{}",
        out.content.text
    );
    assert!(crate::testutil::card_text(home.path(), "user", "a").is_none());

    // A move into `user`, allowed for the session, lands at the new path.
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/tz" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": true, "scope": "session" }));
    let out = plugin
        .call_tool(
            "write",
            write("@rumu:/cards/user/tz", Some("@rumu:/cards/self/tz")),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "user", "tz").is_some());
    assert!(crate::testutil::card_text(home.path(), "main", "tz").is_none());
    let request = host.decisions.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request["kind"], "card_write");
    assert_eq!(request["deck"], "user");
    assert_eq!(request["move_from"], "self/tz");

    // The session grant covers the next write without a prompt.
    let asked = host.decisions.lock().unwrap().len();
    let out = plugin
        .call_tool("write", write("@rumu:/cards/user/b", None), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(host.decisions.lock().unwrap().len(), asked);
}

/// Deleting a card: read first, pinned ones refused, links free; another
/// deck's card asks like a write; the branch is not told of its own delete.
#[tokio::test]
async fn cards_are_deleted_after_a_read() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "user", None, &[]);
    crate::testutil::put_card(home.path(), "main", "n", "note\n");
    crate::testutil::put_card(
        home.path(),
        "main",
        "about",
        "---\npinned: true\n---\ncharter\n",
    );
    crate::testutil::put_card(home.path(), "user", "u", "user fact\n");
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let user = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "user")
        .unwrap()
        .id()
        .to_owned();
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: user,
            at: "/cards/user".into(),
        });
    let cx = host.cx();
    let delete = |path: &str| json!({ "path": path });

    // Not read yet: refused.
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards/self/n"), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("read it first"),
        "{}",
        out.content.text
    );
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/n" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards/self/n.md"), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "main", "n").is_none());
    let patch = out.content.decode::<patch::FilePatch>().unwrap();
    assert_eq!(patch.kind, patch::PatchKind::Delete);
    assert!(patch.file.is_none());
    assert!(host.decisions.lock().unwrap().is_empty());
    // Its own delete is seen: no "was deleted" notice follows.
    flush_output(&host, "call-1", &out);
    let seen = host.cx().index().unwrap().all::<usage::CardUpdate>();
    assert!(
        seen.iter()
            .any(|e| e.value.path == "n" && e.value.rev == "deleted")
    );

    // Pinned cards are the user's.
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards/self/about"), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("pinned"),
        "{}",
        out.content.text
    );

    // A link holds nothing to lose: deleted without a read.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/l", "content": "---\nlink: @rumu:/cards/self/about\n---\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards/self/l"), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "main", "l").is_none());

    // Another deck's card asks; declined, it stays.
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/user/u" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": false }));
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards/user/u"), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("declined"),
        "{}",
        out.content.text
    );
    let request = host.decisions.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request["kind"], "card_delete");
    assert_eq!(request["content"], "user fact\n");
    assert!(crate::testutil::card_text(home.path(), "user", "u").is_some());

    // Directories and other roots are refused.
    let out = plugin
        .call_tool("delete", delete("@rumu:/cards"), &cx)
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("is a directory"));
    let out = plugin
        .call_tool("delete", delete("@rumu:/sessions"), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("cannot be deleted"),
        "{}",
        out.content.text
    );
}

/// Card paths cannot leave their deck, and a move out of another deck needs
/// the user's approval.
#[tokio::test]
async fn card_paths_cannot_escape_a_mount() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "other", None, &[]);
    crate::testutil::put_card(home.path(), "other", "x", "private\n");
    let (plugin, _host, _cx) = card_plugin_for(home.path());
    let other = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "other")
        .unwrap()
        .id()
        .to_owned();
    let host = FakeRuntime::arc();
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: other,
            at: "/cards/other".into(),
        });
    let cx = host.cx();
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": "@rumu:/cards/self/.git/config" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/stolen", "content": "x\n", "from": "self/../other/x" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "other", "x").is_some());
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self", "content": "x\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(crate::testutil::card_text(home.path(), "main", "").is_none());
}

/// `read` returns exactly the editable view (the header is structured data),
/// so writing it back verbatim preserves the frontmatter.
#[tokio::test]
async fn read_output_writes_back_verbatim() {
    let home = card_home();
    crate::testutil::put_card(
        home.path(),
        "main",
        "r",
        "---\ndescription: round trip\nkeywords: [alpha, beta]\nowner: me\n---\nbody\n",
    );
    let (plugin, host, cx) = card_plugin_for(home.path());
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/r" }), &cx)
        .await
        .unwrap();
    assert!(!read.is_error, "{}", read.content.text);
    let header: card::CardReadHeader = read.content.decode().expect("read header");
    assert_eq!(header.deck, crate::testutil::MAIN_DECK);
    assert_eq!(header.path, "r");
    assert!(!read.content.text.contains("· id"), "{}", read.content.text);
    flush_output(&host, "call-1", &read);

    let write = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/self/r", "content": read.content.text }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!write.is_error, "{}", write.content.text);
    let after = deck_card(home.path(), "r");
    assert_eq!(after.description.as_deref(), Some("round trip"));
    assert_eq!(after.keywords(), ["alpha", "beta"]);
    assert_eq!(after.meta.get("owner"), Some(&json!("me")));
    assert_eq!(after.body, "body\n");
}

/// Links are written by path and store the target's deck id and path. A move
/// leaves the link dangling: the card is new at the destination.
#[tokio::test]
async fn links_store_the_target_and_go_dangling_on_a_move() {
    let home = card_home();
    crate::testutil::create_deck(home.path(), "other", None, &[]);
    crate::testutil::put_card(home.path(), "other", "t", "target body\n");
    crate::testutil::put_card(home.path(), "main", "s", "plain\n");
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let decks = plugin.decks();
    let main = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .id()
        .to_owned();
    let other = decks
        .list()
        .into_iter()
        .find(|d| d.meta.name == "other")
        .unwrap()
        .id()
        .to_owned();
    decks.remember(
        rumu_cards::deck::Action::Link,
        rumu_cards::deck::Scope::Always,
        "",
        &main,
        &other,
    );
    *host.state.lock().unwrap() = PathState {
        workspaces: vec![],
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: main.clone(),
        mounts: vec![
            rumu_core::CardMount {
                deck: main.clone(),
                at: "/cards/self".into(),
            },
            rumu_core::CardMount {
                deck: other.clone(),
                at: "/cards/other".into(),
            },
        ],
        ..Default::default()
    };
    let cx = host.cx();

    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/link",
                "content": "---\nlink: @rumu:/cards/other/t\n---\nignored\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    flush_output(&host, "call-1", &out);
    let text = crate::testutil::card_text(home.path(), "main", "link").unwrap();
    assert!(text.contains(&format!("link: {other}:t")), "{text}");

    // The target moves; the link goes dangling (nothing follows the card).
    crate::testutil::move_card(home.path(), "other", "t", "tt");
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/link" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no longer exists"),
        "{}",
        out.content.text
    );

    // An unknown path is refused.
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/bad",
                "content": "---\nlink: @rumu:/cards/self/nope\n---\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no card at"),
        "{}",
        out.content.text
    );

    // A self-link is refused.
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/s" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/s",
                "content": "---\nlink: @rumu:/cards/self/s\n---\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("link to itself"),
        "{}",
        out.content.text
    );
}

/// History is per path: a moved card starts over at its new path.
#[tokio::test]
async fn history_lists_versions_by_path() {
    let home = card_home();
    crate::testutil::put_card(home.path(), "main", "a", "one\n");
    let (plugin, host, cx) = card_plugin_for(home.path());
    let read = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/a" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &read);
    let moved = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/sub/b",
                "content": "two\n",
                "from": "@rumu:/cards/self/a",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!moved.is_error, "{}", moved.content.text);
    flush_output(&host, "call-1", &moved);

    let out = plugin
        .call_tool(
            "list",
            json!({ "path": "@rumu:/history/cards/self/sub/b" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("move"), "{}", out.content.text);
    assert!(
        out.content.text.contains("self/sub/b"),
        "{}",
        out.content.text
    );

    let commit = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "main")
        .unwrap()
        .commits(1)
        .unwrap()
        .remove(0);
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": format!("@rumu:/history/cards/self/sub/b@{}", commit.short) }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    // The new path has only the move: a full version, not a diff from the
    // old card's text.
    assert!(out.content.text.contains("two"), "{}", out.content.text);
    assert!(!out.content.text.contains("-one"), "{}", out.content.text);
}

// ---------- workspaces and the session's project ----------

const PROJECT_ID: &str = "0193a2f1-0000-7000-8000-00000000c3c3";
const PROJECT2_ID: &str = "0193a2f1-0000-7000-8000-00000000c4c4";

/// A temp home with `main`, a project deck `proj`, and a registry of
/// `app` (read-only), `ci` (ssh) and `proj` (local). The project mounts
/// `app` then `proj`; the session is in it.
fn project_setup() -> (
    tempfile::TempDir,
    WorkspacePlugin,
    std::sync::Arc<FakeRuntime>,
    rumu_core::Cx,
    rumu_core::Workspaces,
) {
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "proj",
        Some(&format!("id: {PROJECT_ID}\nname: proj\nproject: true\n")),
        &[],
    );
    let proj_dir = home.path().join("proj-ws");
    let app_dir = home.path().join("app-ws");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::create_dir_all(&app_dir).unwrap();
    let registry = rumu_core::Workspaces::with_projects(
        vec![
            ws("proj", &proj_dir.to_string_lossy()),
            Workspace::new(
                "app".into(),
                None,
                app_dir.to_string_lossy().into_owned(),
                true,
            ),
            Workspace::new(
                "ci".into(),
                Some("gw.example".into()),
                "/srv/ci".into(),
                false,
            ),
        ],
        rumu_core::ProjectWorkspaces::from([(
            PROJECT_ID.to_owned(),
            vec!["app".into(), "proj".into()],
        )]),
    );
    let plugin = deck_plugin_with(home.path(), registry.clone());
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = PathState {
        workspaces: registry.list(),
        mounted: registry.project(PROJECT_ID),
        project: Some(PROJECT_ID.into()),
        default_workspace: Some("app".into()),
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        mounts: vec![
            rumu_core::CardMount {
                deck: crate::testutil::TEST_ASSISTANT.into(),
                at: "/cards/self".into(),
            },
            rumu_core::CardMount {
                deck: PROJECT_ID.into(),
                at: rumu_core::PROJECT_MOUNT.into(),
            },
        ],
        ..Default::default()
    };
    let cx = host.cx();
    (home, plugin, host, cx, registry)
}

#[tokio::test]
async fn workspace_registry_reads_and_writes() {
    let (home, plugin, host, cx, registry) = project_setup();

    // The registry listing: every workspace, location, ro, mounted and default.
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/workspaces" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let lines: Vec<&str> = out.content.text.lines().collect();
    assert_eq!(lines.len(), 3, "{}", out.content.text);
    assert!(lines[0].starts_with("* app local "), "{}", lines[0]);
    assert!(lines[0].ends_with(" ro  (mounted)"), "{}", lines[0]);
    assert!(
        lines[1].starts_with("  ci ssh:gw.example /srv/ci rw"),
        "{}",
        lines[1]
    );
    assert!(lines[2].starts_with("  proj local "), "{}", lines[2]);
    assert!(lines[2].ends_with(" rw  (mounted)"), "{}", lines[2]);
    assert!(!out.content.text.contains("attached"));

    // Reading an entry returns its YAML; unknown names are refused.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/workspaces/ci" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("root: /srv/ci"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("ssh: gw.example"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/workspaces/app" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("read_only: true"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/workspaces/ghost" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no workspace `ghost`"),
        "{}",
        out.content.text
    );

    // Creating an entry needs no approval and saves it.
    let new_dir = home.path().join("new-ws");
    std::fs::create_dir(&new_dir).unwrap();
    let created = format!("root: {}\n", new_dir.display());
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new", "content": created.clone() }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(
        registry.get("new").unwrap().root.as_str(),
        new_dir.to_string_lossy()
    );
    assert!(host.decisions.lock().unwrap().is_empty());

    // A local root must be an existing directory.
    let missing = format!("root: {}\n", home.path().join("ghost").display());
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new2", "content": missing }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("not an existing directory"),
        "{}",
        out.content.text
    );
    assert!(registry.get("new2").is_none());

    // Identical content is a no-op.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new", "content": created }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("unchanged"),
        "{}",
        out.content.text
    );
    assert!(host.decisions.lock().unwrap().is_empty());

    // Changing an existing entry asks the user (`workspace`).
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": true, "scope": "once" }));
    let changed = format!("root: {}\nread_only: true\n", new_dir.display());
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new", "content": changed }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(registry.get("new").unwrap().read_only);
    {
        let decisions = host.decisions.lock().unwrap();
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        let request = &decisions[0];
        assert_eq!(request["kind"], "workspace_change");
        assert_eq!(request["name"], "new");
        assert_eq!(request["from_id"], crate::testutil::TEST_ASSISTANT);
        assert!(
            request["before"].as_str().unwrap().contains("root:"),
            "{request}"
        );
        assert!(
            request["after"]
                .as_str()
                .unwrap()
                .contains("read_only: true"),
            "{request}"
        );
    }

    // A declined change leaves the entry alone.
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": false }));
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new", "content": format!("root: {}\n", new_dir.display()) }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("declined"),
        "{}",
        out.content.text
    );
    assert!(registry.get("new").unwrap().read_only);

    // Nobody answering is no approval either, and the model hears which.
    host.answers
        .lock()
        .unwrap()
        .push_back(serde_json::Value::Null);
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/workspaces/new", "content": format!("root: {}\n", new_dir.display()) }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("did not answer within 10 min"),
        "{}",
        out.content.text
    );
    assert!(registry.get("new").unwrap().read_only);

    // Edit changes an existing entry through the same approval path.
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": true }));
    let out = plugin
        .call_tool(
            "edit",
            json!({
                "path": "@rumu:/workspaces/new",
                "old_string": "read_only: true",
                "new_string": "read_only: false",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(!registry.get("new").unwrap().read_only);
    let out = plugin
        .call_tool(
            "edit",
            json!({
                "path": "@rumu:/workspaces/ghost",
                "old_string": "a",
                "new_string": "b",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
}

/// Deleting a registry entry asks the user and unmounts it everywhere, as
/// the web's `workspace.delete` does.
#[tokio::test]
async fn workspace_entries_are_deleted_with_approval() {
    let (_home, plugin, host, cx, registry) = project_setup();
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": false }));
    let args = json!({ "path": "@rumu:/workspaces/proj" });
    let out = plugin.call_tool("delete", args.clone(), &cx).await.unwrap();
    assert!(
        out.is_error && out.content.text.contains("declined"),
        "{}",
        out.content.text
    );
    assert!(registry.get("proj").is_some());
    let request = host.decisions.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request["kind"], "workspace_delete");
    assert!(request["before"].as_str().unwrap().contains("root:"));

    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": true }));
    let out = plugin.call_tool("delete", args, &cx).await.unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(registry.get("proj").is_none());
    assert_eq!(registry.project(PROJECT_ID), ["app"]);

    let out = plugin
        .call_tool("delete", json!({ "path": "@rumu:/workspaces/nope" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    let out = plugin
        .call_tool(
            "delete",
            json!({ "path": "@rumu:/project/workspaces" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("cannot be deleted"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn project_workspaces_are_read_before_write() {
    let (_home, plugin, host, cx, registry) = project_setup();

    // The root lists the project directory when the session is in one.
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("workspaces/"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("project/"),
        "{}",
        out.content.text
    );

    // Reading returns the ordered list and records its content hash.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/project/workspaces" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "- app\n- proj\n");
    assert_eq!(
        out.effects[0].data.as_ref().unwrap().tag,
        "project.workspaces.read"
    );
    flush_output(&host, "call-1", &out);

    // A write based on that read reorders the project's workspaces.
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/project/workspaces",
                "content": "- proj\n- app\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(
        registry.project(PROJECT_ID),
        vec!["proj".to_owned(), "app".to_owned()]
    );
    // The write is the base of the next one, like a read.
    assert_eq!(
        out.effects[0].data.as_ref().unwrap().tag,
        "project.workspaces.read"
    );

    // Without that marker (another session's change, say) the old read is
    // stale: the refusal returns the current list.
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/project/workspaces",
                "content": "- app\n- proj\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("changed since you read it"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("- proj\n- app"),
        "{}",
        out.content.text
    );
    assert_eq!(
        registry.project(PROJECT_ID),
        vec!["proj".to_owned(), "app".to_owned()]
    );

    // A fresh read makes a write valid again.
    let fresh = host.cx();
    let out = plugin
        .call_tool(
            "read",
            json!({ "path": "@rumu:/project/workspaces" }),
            &fresh,
        )
        .await
        .unwrap();
    flush_output(&host, "call-1", &out);
    let fresh = host.cx();
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/project/workspaces",
                "content": "- app\n- proj\n",
            }),
            &fresh,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);

    // `edit` applies to the current text, with no read required.
    let fresh = host.cx();
    let out = plugin
        .call_tool(
            "edit",
            json!({
                "path": "@rumu:/project/workspaces",
                "old_string": "- app\n- proj",
                "new_string": "- proj\n- app",
            }),
            &fresh,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert_eq!(
        registry.project(PROJECT_ID),
        vec!["proj".to_owned(), "app".to_owned()]
    );

    // Unknown names are refused, and no approval is involved.
    let fresh = host.cx();
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/project/workspaces",
                "content": "- ghost\n",
            }),
            &fresh,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(host.decisions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn project_config_needs_a_project() {
    let home = card_home();
    let (plugin, _host, cx) = card_plugin_for(home.path());
    for tool in ["read", "list"] {
        let out = plugin
            .call_tool(tool, json!({ "path": "@rumu:/project/workspaces" }), &cx)
            .await
            .unwrap();
        assert!(
            out.is_error && out.content.text.contains("in no project"),
            "{tool}: {}",
            out.content.text
        );
    }
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/project/workspaces", "content": "[]" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("in no project"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/" }), &cx)
        .await
        .unwrap();
    assert!(
        !out.content.text.contains("project/"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn workspace_use_lists_the_projects_workspaces_first() {
    let (_home, plugin, _host, cx, _registry) = project_setup();
    let out = plugin
        .call_tool("workspace_use", json!({}), &cx)
        .await
        .unwrap();
    let lines: Vec<&str> = out.content.text.lines().collect();
    assert!(lines[0].starts_with("* app local "), "{}", lines[0]);
    assert!(lines[0].ends_with("(mounted)"), "{}", lines[0]);
    assert!(lines[1].starts_with("  proj local "), "{}", lines[1]);
    assert!(lines[1].ends_with("(mounted)"), "{}", lines[1]);
    assert!(lines[2].starts_with("  ci ssh:gw.example"), "{}", lines[2]);
    assert!(lines[3].starts_with("  @rumu"), "{}", lines[3]);
    assert!(!out.content.text.contains("attached"));
}

#[tokio::test]
async fn sessions_belong_to_their_project_or_assistant_deck() {
    const OTHER: &str = "0193a2f1-0000-7000-8000-00000000b2b2";
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "proj",
        Some(&format!("id: {PROJECT_ID}\nname: proj\nproject: true\n")),
        &[],
    );
    crate::testutil::create_deck(
        home.path(),
        "other",
        Some(&format!("id: {OTHER}\nname: other\nassistant: true\n")),
        &[],
    );
    let plugin = deck_plugin(home.path());
    let host = FakeRuntime::arc();
    let session = |id: &str, assistant: &str, project: Option<&str>| rumu_core::SessionInfo {
        id: rumu_core::SessionId::from(id),
        title: id.into(),
        active_branch: Default::default(),
        created_at: 1,
        updated_at: 2,
        touched_at: 2,
        seen_at: None,
        assistant: assistant.into(),
        project: project.map(str::to_owned),
    };
    host.sessions.lock().unwrap().extend([
        session(
            "01SPROJECTSESSION000000000000",
            crate::testutil::MAIN_DECK,
            Some(PROJECT_ID),
        ),
        session("01SOTHERSESSION0000000000000", OTHER, None),
    ]);
    // The project is mounted; the other assistant's deck is not.
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: PROJECT_ID.into(),
            at: rumu_core::PROJECT_MOUNT.into(),
        });
    let cx = host.cx();
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/sessions" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("01SPROJECTSESSION000000000000"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("01SOTHERSESSION0000000000000"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "list",
            json!({ "path": "@rumu:/sessions/by-assistant" }),
            &cx,
        )
        .await
        .unwrap();
    assert_eq!(
        out.content.text,
        format!(
            "@rumu:/sessions/by-assistant/{}  main (you)",
            crate::testutil::MAIN_DECK
        )
    );

    // Leaving the project hides its sessions; mounting the other assistant
    // shows its own.
    {
        let mut state = host.state.lock().unwrap();
        state.mounts.retain(|m| m.at != rumu_core::PROJECT_MOUNT);
        state.mounts.push(rumu_core::CardMount {
            deck: OTHER.into(),
            at: "/cards/other".into(),
        });
    }
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/sessions" }), &cx)
        .await
        .unwrap();
    assert!(
        !out.content.text.contains("01SPROJECTSESSION000000000000"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("01SOTHERSESSION0000000000000"),
        "{}",
        out.content.text
    );
    let out = plugin
        .call_tool(
            "list",
            json!({ "path": format!("@rumu:/sessions/by-assistant/{}", crate::testutil::MAIN_DECK) }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no visible sessions"),
        "{}",
        out.content.text
    );
}

/// A home with `main` and two project decks, and a registry whose first
/// project mounts `app`.
fn mount_setup() -> (
    tempfile::TempDir,
    WorkspacePlugin,
    std::sync::Arc<FakeRuntime>,
    rumu_core::Cx,
    rumu_core::Workspaces,
) {
    let home = tempfile::tempdir().unwrap();
    crate::testutil::main_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "proj",
        Some(&format!("id: {PROJECT_ID}\nname: proj\nproject: true\n")),
        &[],
    );
    crate::testutil::create_deck(
        home.path(),
        "proj2",
        Some(&format!("id: {PROJECT2_ID}\nname: proj2\nproject: true\n")),
        &[],
    );
    let app_dir = home.path().join("app-ws");
    std::fs::create_dir(&app_dir).unwrap();
    let registry = rumu_core::Workspaces::with_projects(
        vec![Workspace::new(
            "app".into(),
            None,
            app_dir.to_string_lossy().into_owned(),
            false,
        )],
        rumu_core::ProjectWorkspaces::from([(PROJECT_ID.to_owned(), vec!["app".into()])]),
    );
    let plugin = deck_plugin_with(home.path(), registry.clone());
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = PathState {
        workspaces: registry.list(),
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        mounts: vec![rumu_core::CardMount {
            deck: crate::testutil::TEST_ASSISTANT.into(),
            at: "/cards/self".into(),
        }],
        ..Default::default()
    };
    let cx = host.cx();
    (home, plugin, host, cx, registry)
}

#[tokio::test]
async fn card_mount_moves_between_projects_and_creates_one() {
    use rumu_cards::deck::{Action, Scope};

    let (_home, plugin, host, cx, registry) = mount_setup();
    let main = crate::testutil::TEST_ASSISTANT;

    // The listing marks project decks like assistants.
    let out = plugin
        .call_tool("card_mount", json!({}), &cx)
        .await
        .unwrap();
    let proj_line = out
        .content
        .text
        .lines()
        .find(|l| l.starts_with("proj ("))
        .unwrap_or_else(|| panic!("{}", out.content.text));
    assert!(proj_line.contains("  project"), "{proj_line}");
    assert!(
        out.content.text.lines().any(|l| l.contains("assistant")),
        "{}",
        out.content.text
    );

    // Only project decks mount at `/cards/project`.
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": main, "at": "/cards/project" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("only project decks"),
        "{}",
        out.content.text
    );

    // A project mount carries the workspaces, default first, and a
    // `WorkspaceUse` marker for its first one.
    plugin
        .decks()
        .remember(Action::Mount, Scope::Always, "", main, PROJECT_ID);
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": PROJECT_ID, "at": "project" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content
            .text
            .contains("Its workspaces: `app`; `app` is now the default workspace."),
        "{}",
        out.content.text
    );
    let mount: rumu_core::CardMount = out.effects[0].decode().unwrap();
    assert_eq!(mount.deck, PROJECT_ID);
    assert_eq!(mount.at, rumu_core::PROJECT_MOUNT);
    let use_: WorkspaceUse = out.effects[1].decode().unwrap();
    assert_eq!(use_.name, "app");
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: PROJECT_ID.into(),
            at: rumu_core::PROJECT_MOUNT.into(),
        });

    // Mounting another project there replaces it instead of refusing.
    plugin
        .decks()
        .remember(Action::Mount, Scope::Always, "", main, PROJECT2_ID);
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": PROJECT2_ID, "at": "project" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let mount: rumu_core::CardMount = out.effects[0].decode().unwrap();
    assert_eq!(mount.deck, PROJECT2_ID);
    let use_: WorkspaceUse = out.effects[1].decode().unwrap();
    assert_eq!(use_.name, SCRATCH_WORKSPACE, "it mounts no workspace");
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: PROJECT2_ID.into(),
            at: rumu_core::PROJECT_MOUNT.into(),
        });

    // Unmounting the project leaves it and resets the default to `@scratch`.
    let out = plugin
        .call_tool("card_mount", json!({ "at": "project" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("leaves its project"),
        "{}",
        out.content.text
    );
    let unmount: rumu_core::CardUnmount = out.effects[0].decode().unwrap();
    assert_eq!(unmount.at, rumu_core::PROJECT_MOUNT);
    let use_: WorkspaceUse = out.effects[1].decode().unwrap();
    assert_eq!(use_.name, SCRATCH_WORKSPACE);

    // `create` is only valid with `at: "project"`, never with a deck.
    for (args, needle) in [
        (
            json!({ "deck": main, "create": "fresh", "at": "project" }),
            "cannot be combined",
        ),
        (json!({ "create": "fresh" }), "needs `at"),
        (
            json!({ "create": "fresh", "at": "/cards/user" }),
            "needs `at",
        ),
        (
            json!({ "create": " ", "at": "project" }),
            "invalid project name",
        ),
    ] {
        let out = plugin.call_tool("card_mount", args, &cx).await.unwrap();
        assert!(
            out.is_error && out.content.text.contains(needle),
            "{}",
            out.content.text
        );
    }

    // Creating a project asks (`create`, target `*`) and moves into it.
    host.answers
        .lock()
        .unwrap()
        .push_back(json!({ "allow": true }));
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "create": "fresh", "at": "project" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("mounts no workspace"),
        "{}",
        out.content.text
    );
    {
        let decisions = host.decisions.lock().unwrap();
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        assert_eq!(decisions[0]["kind"], "project_create");
        assert_eq!(decisions[0]["name"], "fresh");
        assert_eq!(decisions[0]["from_id"], main);
    }
    let fresh = plugin
        .decks()
        .list()
        .into_iter()
        .find(|d| d.meta.name == "fresh")
        .expect("the project deck was created");
    assert!(fresh.project());
    assert!(registry.project(fresh.id()).is_empty());
    let mount: rumu_core::CardMount = out.effects[0].decode().unwrap();
    assert_eq!(mount.deck, fresh.id());
    let use_: WorkspaceUse = out.effects[1].decode().unwrap();
    assert_eq!(use_.name, SCRATCH_WORKSPACE);
}

#[tokio::test]
async fn the_sessions_project_deck_is_writable_without_approval() {
    let (home, plugin, host, cx, _registry) = project_setup();
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/project/y",
                "content": "---\ntitle: Y\n---\nnew\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        host.decisions.lock().unwrap().is_empty(),
        "the project deck is the session's own"
    );
    assert!(
        crate::testutil::card_text(home.path(), "proj", "y")
            .unwrap()
            .contains("new")
    );

    // The mount listing says nothing is approved automatically for mount,
    // but writing it does not ask.
    let out = plugin
        .call_tool("card_mount", json!({}), &cx)
        .await
        .unwrap();
    let proj_line = out
        .content
        .text
        .lines()
        .find(|l| l.starts_with("proj ("))
        .unwrap_or_else(|| panic!("{}", out.content.text));
    assert!(proj_line.contains("asks the user to mount"), "{proj_line}");
    assert!(!proj_line.contains("write"), "{proj_line}");
}

// ---------- private decks ----------

const PRIVATE_DECK: &str = "0193a2f1-0000-7000-8000-00000000c1c1";
const BOSS_DECK: &str = "0193a2f1-0000-7000-8000-00000000c2c2";

fn hidden_deck(home: &Path) -> rumu_cards::Deck {
    crate::testutil::create_deck(
        home,
        "hidden",
        Some(&format!(
            "id: {PRIVATE_DECK}\nname: hidden\nprivate: true\n"
        )),
        &[("t", "target body\n")],
    )
}

/// A private deck is absent from the listing and cannot be mounted by id,
/// even with an always-allow grant: the refusal looks like an unknown id and
/// asks the user nothing.
#[tokio::test]
async fn private_decks_are_unlisted_and_unmountable() {
    let home = card_home();
    let hidden = hidden_deck(home.path());
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let decks = plugin.decks();
    let main = crate::testutil::MAIN_DECK;
    let cx = host.cx();

    let out = plugin
        .call_tool("card_mount", json!({}), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        !out.content.text.contains("hidden") && !out.content.text.contains(&hidden.meta.id),
        "{}",
        out.content.text
    );

    decks.remember(
        rumu_cards::deck::Action::Mount,
        rumu_cards::deck::Scope::Always,
        "",
        main,
        &hidden.meta.id,
    );
    let out = plugin
        .call_tool(
            "card_mount",
            json!({ "deck": hidden.meta.id, "at": "/cards/hidden" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no deck with id"),
        "{}",
        out.content.text
    );
    assert!(
        host.decisions.lock().unwrap().is_empty(),
        "a private mount is refused before asking the user"
    );

    // Mounting it on the path (a user action) makes it visible.
    host.state
        .lock()
        .unwrap()
        .mounts
        .push(rumu_core::CardMount {
            deck: hidden.meta.id.clone(),
            at: "/cards/hidden".into(),
        });
    let out = plugin
        .call_tool("card_mount", json!({}), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("hidden") && out.content.text.contains(&hidden.meta.id),
        "{}",
        out.content.text
    );
}

/// A private deck listed in the session assistant's `deck.yaml` mounts is
/// visible to its sessions.
#[tokio::test]
async fn private_decks_are_visible_through_the_assistant_mounts() {
    let home = card_home();
    let hidden = hidden_deck(home.path());
    crate::testutil::create_deck(
        home.path(),
        "boss",
        Some(&format!(
            "id: {BOSS_DECK}\nname: boss\nassistant: true\nmounts:\n  - {{ deck: {}, at: /cards/hidden }}\n",
            hidden.meta.id
        )),
        &[],
    );
    let (plugin, host, _cx) = card_plugin_for(home.path());
    *host.state.lock().unwrap() = PathState {
        workspaces: vec![],
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: BOSS_DECK.into(),
        mounts: vec![rumu_core::CardMount {
            deck: BOSS_DECK.into(),
            at: "/cards/self".into(),
        }],
        ..Default::default()
    };
    let cx = host.cx();
    let out = plugin
        .call_tool("card_mount", json!({}), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("hidden") && out.content.text.contains(&hidden.meta.id),
        "{}",
        out.content.text
    );
}

/// Cross-deck links into a private deck are refused before approval, links
/// within it work, and a link that predates the privacy only dangles.
#[tokio::test]
async fn private_decks_refuse_cross_deck_links() {
    let home = card_home();
    let hidden = hidden_deck(home.path());
    crate::testutil::put_card(home.path(), "main", "plain", "plain\n");
    let (plugin, host, _cx) = card_plugin_for(home.path());
    let decks = plugin.decks();
    let main = crate::testutil::MAIN_DECK;
    *host.state.lock().unwrap() = PathState {
        workspaces: vec![],
        default_workspace: Some(RUMU_WORKSPACE.into()),
        assistant: main.into(),
        mounts: vec![
            rumu_core::CardMount {
                deck: main.into(),
                at: "/cards/self".into(),
            },
            rumu_core::CardMount {
                deck: hidden.meta.id.clone(),
                at: "/cards/hidden".into(),
            },
        ],
        ..Default::default()
    };
    let cx = host.cx();
    for action in [
        rumu_cards::deck::Action::Link,
        rumu_cards::deck::Action::Write,
    ] {
        decks.remember(
            action,
            rumu_cards::deck::Scope::Always,
            "",
            main,
            &hidden.meta.id,
        );
    }

    // Another deck cannot link to it, and no approval is requested.
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/self/link",
                "content": "---\nlink: @rumu:/cards/hidden/t\n---\nignored\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("private deck"),
        "{}",
        out.content.text
    );
    assert!(
        host.decisions.lock().unwrap().is_empty(),
        "the refusal comes before the approval prompt"
    );

    // Within the private deck itself a link is fine.
    let out = plugin
        .call_tool(
            "write",
            json!({
                "path": "@rumu:/cards/hidden/own",
                "content": "---\nlink: @rumu:/cards/hidden/t\n---\nignored\n",
            }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = crate::testutil::card_text(home.path(), "hidden", "own").unwrap();
    assert!(
        text.contains(&format!("link: {}:t", hidden.meta.id)),
        "{text}"
    );

    // A link written before the deck became private now shows as dangling.
    crate::testutil::put_card(
        home.path(),
        "main",
        "old",
        format!("---\nlink: {}:t\n---\nignored\n", hidden.meta.id),
    );
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/self/old" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no longer exists"),
        "{}",
        out.content.text
    );
}
