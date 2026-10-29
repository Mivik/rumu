//! Tests for the workspace plugin: `<ws>:<path>` resolution, the file/bash
//! tools over local backends (absolute paths included), `workspace_use`, the
//! `@rumu` virtual workspace (`/cards` reads/writes with validation and
//! read-before-write, `/sessions` views, `/task` reads, unknown roots), AGENTS.md
//! injection and the config.yaml loader. The ssh backend's command construction
//! is tested in `backend.rs` (no real ssh).

use super::*;
use crate::testutil::{FakeRuntime, fixture_home};
use rumu_cards::store::FsCardStore;
use rumu_cards::{card, library, usage};
use rumu_core::{
    Node, NodeBody, PathState, RUMU_WORKSPACE, Structured, Tagged, ToolOutput, WorkspaceUse,
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
    std::fs::create_dir_all(home.path().join("cards")).unwrap();
    let plugin = WorkspacePlugin::new(home.path(), WorkspaceCatalog::default());
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

// ---------- resolution ----------

#[tokio::test]
async fn address_resolution_errors() {
    let home = tempfile::tempdir().unwrap();
    let plugin = WorkspacePlugin::new(home.path(), WorkspaceCatalog::default());
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

#[tokio::test]
async fn read_write_edit_list_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (plugin, _host, cx, _home) = setup(dir.path(), false);

    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "a/b.txt", "content": "l1\nl2\nl3\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content
            .text
            .starts_with("[proj] wrote proj:a/b.txt (9 bytes)"),
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
    assert_eq!(out.content.text, "[proj] a/");
    let out = plugin
        .call_tool("list", json!({ "path": "proj:" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "[proj] a/");
    let out = plugin
        .call_tool("list", json!({ "path": "a" }), &cx)
        .await
        .unwrap();
    assert_eq!(out.content.text, "[proj] b.txt");

    let out = plugin
        .call_tool("read", json!({ "path": "missing.txt" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.starts_with("[proj] "),
        "{}",
        out.content.text
    );
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
        out.content.text.starts_with("[proj] exit code: 0\n"),
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

    // Output truncation around ~20k chars.
    let out = plugin
        .call_tool("bash", json!({ "command": "seq 1 8000" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("[truncated"),
        "{} chars",
        out.content.text.len()
    );
    assert!(out.content.text.chars().count() < 22_000);

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
    let catalog = WorkspaceCatalog {
        workspaces: vec![ws("proj", "/p/rumu"), prod],
        default: Some("proj".into()),
    };
    let plugin = WorkspacePlugin::new(home.path(), catalog.clone());
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() =
        state_with(catalog.workspaces.clone(), catalog.default.as_deref());
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
        out.content.text.starts_with("[prod] default workspace set"),
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
    std::fs::create_dir_all(dir.path().join("cards")).unwrap();
    dir
}

fn card_plugin_for(home: &Path) -> (WorkspacePlugin, std::sync::Arc<FakeRuntime>, rumu_core::Cx) {
    let plugin = WorkspacePlugin::new(home, WorkspaceCatalog::default());
    let host = FakeRuntime::arc();
    *host.state.lock().unwrap() = state_with(vec![], Some(RUMU_WORKSPACE));
    let cx = host.cx();
    (plugin, host, cx)
}

/// `@rumu:/cards/x` ≡ `@rumu:/cards/x.md`; with `@rumu` as the branch default,
/// `/cards/x` and `cards/x` work too.
#[tokio::test]
async fn card_paths_are_equivalent_in_every_form() {
    let home = card_home();
    std::fs::write(
        home.path().join("cards/m.md"),
        "---\ntitle: Old\n---\nbody v1\n",
    )
    .unwrap();
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let mut effects = Vec::new();
    for path in [
        "@rumu:/cards/m",
        "@rumu:/cards/m.md",
        "@rumu:cards/m",
        "/cards/m",
        "cards/m",
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
    assert_eq!(payload.id, "m");
    assert_eq!(payload.op, usage::CardOp::Read);
}

/// Other roots and bash on `@rumu` are refused with useful errors.
#[tokio::test]
async fn rumu_workspace_edges() {
    let home = card_home();
    let (plugin, _host, cx) = card_plugin_for(home.path());

    // `list @rumu:` shows the roots.
    let out = plugin.call_tool("list", json!({}), &cx).await.unwrap();
    assert_eq!(out.content.text, "[@rumu] cards/\nsessions/\ntask/");

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
            out.content.text.contains("roots: /cards, /sessions"),
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
            !out.is_error && out.content.text.contains("[@rumu] no sessions yet"),
            "{}",
            out.content.text
        );
    }
}

#[tokio::test]
async fn card_tools_read_write_validate_and_list() {
    let home = card_home();
    std::fs::write(
        home.path().join("cards/m.md"),
        "---\ntitle: Old\ndescription: d\ntags: [a]\nmatch:\n  event: prompt\nowner: x\n---\nbody v1\n",
    )
    .unwrap();
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/m" }), &cx)
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
    assert_eq!(payload.id, "m");
    assert_eq!(payload.op, usage::CardOp::Read);
    assert_eq!(payload.view_rev, card::view_rev_of(&out.content.text));
    assert_eq!(payload.rev.len(), 8);

    // Unknown id: near-miss suggestions, no marker.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/x" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("no card"),
        "{}",
        out.content.text
    );
    assert!(out.effects.is_empty());
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/mm" }), &cx)
        .await
        .unwrap();
    assert!(
        out.is_error && out.content.text.contains("did you mean: m"),
        "{}",
        out.content.text
    );
    assert!(out.effects.is_empty());

    let home = card_home();
    std::fs::write(
        home.path().join("cards/c.md"),
        "---\ntitle: C\n---\nbody v1\n",
    )
    .unwrap();
    let (plugin, host, cx) = card_plugin_for(home.path());

    // Overwrite without a prior read → refused.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/c", "content": "---\ntitle: C\n---\nbody v2\n" }),
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
        std::fs::read_to_string(home.path().join("cards/c.md"))
            .unwrap()
            .contains("v1")
    );

    // Read, then the same write passes.
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/c" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &out);
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/c", "content": "---\ntitle: C\n---\nbody v2\n" }),
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
        out.content.text.starts_with("[@rumu] wrote cards/c.md"),
        "{}",
        out.content.text
    );
    flush_output(&host, "call-1", &out);

    // The write itself records the new view_rev → a chained edit needs no
    // re-read.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/c", "old_string": "v2", "new_string": "v3" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        std::fs::read_to_string(home.path().join("cards/c.md"))
            .unwrap()
            .contains("v3")
    );

    // Someone edits the file behind our back → mismatch error.
    std::fs::write(
        home.path().join("cards/c.md"),
        "---\ntitle: C\n---\nbody EVIL\n",
    )
    .unwrap();
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/c", "old_string": "v3", "new_string": "v4" }),
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
        std::fs::read_to_string(home.path().join("cards/c.md"))
            .unwrap()
            .contains("EVIL")
    );

    // Creating a brand-new card needs no read.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/new/card", "content": "fresh body\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(home.path().join("cards/new/card.md").is_file());

    let home = card_home();
    std::fs::write(
        home.path().join("cards/m.md"),
        "---\ntitle: Old\nmatch:\n  event: prompt\n---\nbody\n",
    )
    .unwrap();
    let (plugin, host, cx) = card_plugin_for(home.path());
    let out = plugin
        .call_tool("read", json!({ "path": "@rumu:/cards/m" }), &cx)
        .await
        .unwrap();
    flush_output(&host, "call-1", &out);

    // `match:` and `scope:` are editable like any other frontmatter key.
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/m", "content": "---\nmatch:\n  event: prompt\nscope: [proj]\n---\nhacked\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = std::fs::read_to_string(home.path().join("cards/m.md")).unwrap();
    assert!(text.contains("hacked"), "{text}");
    assert!(text.contains("match:"), "{text}");
    assert!(text.contains("scope:"), "{text}");
    flush_output(&host, "call-1", &out);

    // Bad types name the offending field.
    for bad in [
        "---\ntitle: [x]\n---\nb\n",
        "---\ndescription: [x]\n---\nb\n",
        "---\n- a\n- b\n---\nb\n",
    ] {
        let out = plugin
            .call_tool(
                "write",
                json!({ "path": "@rumu:/cards/m", "content": bad }),
                &cx,
            )
            .await
            .unwrap();
        assert!(out.is_error, "{bad}");
    }

    // A clean write keeps the `match:` the view carries. A title change moves
    // `rev` (it is part of the render).
    let before = library::load_card(&FsCardStore::home(home.path()), "m")
        .unwrap()
        .unwrap();
    let out = plugin
        .call_tool(
            "write",
            json!({ "path": "@rumu:/cards/m", "content": "---\ntitle: New\nmatch:\n  event: prompt\n---\nnew body\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = std::fs::read_to_string(home.path().join("cards/m.md")).unwrap();
    assert!(text.contains("title: New"), "{text}");
    assert!(text.contains("match:"), "{text}");
    assert!(text.contains("event: prompt"), "{text}");
    assert!(!text.contains("scope:"), "omitted keys are dropped: {text}");
    let after = library::load_card(&FsCardStore::home(home.path()), "m")
        .unwrap()
        .unwrap();
    assert_ne!(before.revision, after.revision, "title is in the render");
    flush_output(&host, "call-1", &out);

    // A description-only edit keeps `rev` (metadata is invisible to the
    // memory plugin) but moves `view_rev`.
    let out = plugin
        .call_tool(
            "edit",
            json!({ "path": "@rumu:/cards/m", "old_string": "title: New", "new_string": "title: New\ndescription: added" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let with_desc = library::load_card(&FsCardStore::home(home.path()), "m")
        .unwrap()
        .unwrap();
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
            json!({ "path": "@rumu:/cards/m", "content": "---\ntitle: New\n---\nplain\n" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let text = std::fs::read_to_string(home.path().join("cards/m.md")).unwrap();
    assert!(!text.contains("match:"), "omitted match is dropped: {text}");

    let home = card_home();
    std::fs::create_dir_all(home.path().join("cards/dev")).unwrap();
    std::fs::write(
        home.path().join("cards/x.md"),
        "---\ntitle: X\ndescription: dx\n---\nsecret body\n",
    )
    .unwrap();
    std::fs::write(home.path().join("cards/dev/y.md"), "plain\n").unwrap();
    let (plugin, _host, cx) = card_plugin_for(home.path());

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/cards" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.starts_with("[@rumu] "),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("@rumu:/cards/dev/y — dev/y"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("@rumu:/cards/x — X — dx"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("secret body"),
        "{}",
        out.content.text
    );

    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/cards/dev" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content.text.contains("@rumu:/cards/dev/y"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("@rumu:/cards/x — X"),
        "{}",
        out.content.text
    );
}

// ---------- config.yaml ----------

fn load_catalog(home: &Path) -> crate::Result<WorkspaceCatalog> {
    crate::load_config(home)?.catalog()
}

#[test]
fn load_catalog_parsing() {
    let cat = load_catalog(&fixture_home()).unwrap();
    assert_eq!(cat.default.as_deref(), Some("proj"));
    let proj = cat.get("proj").unwrap();
    assert!(proj.is_local(), "string shorthand = local");
    assert_eq!(proj.ssh, None);
    assert!(!proj.read_only);
    assert!(
        proj.root.ends_with("code/proj") || proj.root.ends_with("code/proj/"),
        "tilde expanded: {}",
        proj.root
    );

    // Explicit names are validated. `rumu` is an ordinary name now: the
    // internal workspace lives at `@rumu`, which no preset can take.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.yaml"),
        "default: gone\nworkspaces:\n  ok: /r\n  rumu: /x\n",
    )
    .unwrap();
    let cat = load_catalog(dir.path()).unwrap();
    let names: Vec<&str> = cat.workspaces.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["ok", "rumu"], "BTreeMap order");
    assert_eq!(cat.default, None, "default naming no preset is dropped");

    for bad in ["bad/name", "sp ace", "UPPER", "x", "@rumu", "http"] {
        std::fs::write(
            dir.path().join("config.yaml"),
            format!("workspaces:\n  \"{bad}\": /x\n"),
        )
        .unwrap();
        let err = load_catalog(dir.path()).unwrap_err().to_string();
        assert!(err.contains("invalid workspace name"), "{bad}: {err}");
    }

    // Missing file → empty catalog.
    let empty = tempfile::tempdir().unwrap();
    assert!(load_catalog(empty.path()).unwrap().workspaces.is_empty());

    // String shorthand and the map form (`ssh:` + `read_only:`) coexist.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.yaml"),
        "default: app\nworkspaces:\n  proj: ~/code/proj\n  app:\n    ssh: deploy@prod\n    root: /srv/app\n    read_only: true\n",
    )
    .unwrap();
    let cat = load_catalog(dir.path()).unwrap();
    let proj = cat.get("proj").unwrap();
    assert_eq!(proj.ssh, None);
    assert!(
        proj.root.ends_with("code/proj") || proj.root.ends_with("code/proj/"),
        "{}",
        proj.root
    );
    let app = cat.get("app").unwrap();
    assert_eq!(app.ssh.as_deref(), Some("deploy@prod"));
    assert_eq!(app.root, "/srv/app", "remote roots keep their form");
    assert_eq!(app.label(), "ssh:deploy@prod");
    assert!(app.read_only);
    assert_eq!(cat.default.as_deref(), Some("app"));

    // Only the map form exists: a list of roots is not a workspace map.
    std::fs::write(dir.path().join("config.yaml"), "workspaces:\n  - /srv/a\n").unwrap();
    assert!(load_catalog(dir.path()).is_err(), "list form is rejected");

    for (yaml, needle) in [
        // machine-oriented shapes are rejected as unknown fields
        (
            "workspaces:\n  ab:\n    machine: ghost\n    root: /r\n",
            "unknown field `machine`",
        ),
        (
            "machines:\n  prod:\n    kind: ssh\n    host: h\nworkspaces:\n  ab: /r\n",
            "unknown field `machines`",
        ),
        // `trusted:` is an unknown field.
        (
            "workspaces:\n  ab:\n    root: /r\n    trusted: true\n",
            "unknown field `trusted`",
        ),
        (
            "workspaces:\n  ab:\n    kind: ssh\n    host: h\n    root: /r\n",
            "unknown field `kind`",
        ),
        (
            "workspaces:\n  ab:\n    host: h\n    root: /r\n",
            "unknown field `host`",
        ),
        // `name:` only existed for the list form.
        (
            "workspaces:\n  ab:\n    name: alias\n    root: /r\n",
            "unknown field `name`",
        ),
        // ssh validation
        (
            "workspaces:\n  ab:\n    ssh: ''\n    root: /r\n",
            "ssh host must be non-empty",
        ),
        (
            "workspaces:\n  ab:\n    ssh: 'deploy @prod'\n    root: /r\n",
            "ssh host must be non-empty",
        ),
        (
            "workspaces:\n  ab:\n    ssh: deploy@prod\n    root: srv/app\n",
            "ssh `root` must be an absolute path",
        ),
    ] {
        std::fs::write(dir.path().join("config.yaml"), yaml).unwrap();
        let err = load_catalog(dir.path()).unwrap_err().to_string();
        assert!(err.contains(needle), "{yaml}: {err}");
    }
}

#[test]
fn catalog_or_cwd_registers_the_current_directory() {
    // With no config file at all, the cwd becomes a local rw workspace named
    // after the directory and is the default.
    let empty = tempfile::tempdir().unwrap();
    let cat = crate::load_config(empty.path())
        .unwrap()
        .catalog_or_cwd()
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    assert_eq!(cat.workspaces.len(), 1);
    let ws = &cat.workspaces[0];
    assert!(ws.is_local());
    assert_eq!(ws.root.as_str(), cwd.to_string_lossy());
    assert!(!ws.read_only);
    assert_eq!(cat.default.as_deref(), Some(ws.name.as_str()));
    assert!(
        rumu_core::valid_workspace_name(&ws.name),
        "derived an addressable name: {}",
        ws.name
    );

    // A config file disables the fallback.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yaml"), "workspaces:\n  proj: /p\n").unwrap();
    let cat = crate::load_config(dir.path())
        .unwrap()
        .catalog_or_cwd()
        .unwrap();
    assert_eq!(cat.get("proj").unwrap().root, "/p");
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
                content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
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
                reason_ms: None,
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall {
                    call_id: "c1".into(),
                    name: "bash".into(),
                    args: json!({ "command": "ls -la" }),
                }],
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
            },
        ),
        mk(
            "A2",
            Some("E1"),
            8000,
            NodeBody::Assistant {
                content: rumu_core::Content::text("config lives in /etc/rumu"),
                reasoning: vec![],
                reason_ms: None,
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
    std::fs::create_dir_all(home.path().join("cards")).unwrap();
    // Leak the tempdir: the plugin only reads RUMU_HOME lazily and the tests
    // finish long before /tmp cleanup matters.
    let home = home.keep();
    WorkspacePlugin::new(&home, WorkspaceCatalog::default())
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
        Some("[@rumu] @rumu:/sessions/01STESTSSESSION0000000000000")
    );
    assert_stamp(fields.next().unwrap(), 8000);
    assert_eq!(fields.next(), Some("\"config hunting\""), "{line}");

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
                "[@rumu] session 01STESTSSESSION0000000000000 — \"config hunting\" · updated "
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
                content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
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
                reason_ms: None,
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall {
                    call_id: "c1".into(),
                    name: "bash".into(),
                    args: json!({ "command": "printf '<elided>'" }),
                }],
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
                reason_ms: None,
                reasoning_reset: false,
                tool_calls: vec![
                    rumu_core::ToolCall {
                        call_id: "c1".into(),
                        name: "read".into(),
                        args: json!({ "path": "@rumu:/sessions/other/abc" }),
                    },
                    rumu_core::ToolCall {
                        call_id: "c2".into(),
                        name: "search".into(),
                        args: json!({ "query": "x", "scope": "@rumu:/sessions/other" }),
                    },
                    rumu_core::ToolCall {
                        call_id: "c3".into(),
                        name: "bash".into(),
                        args: json!({ "command": "ls" }),
                    },
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
                reason_ms: None,
                reasoning_reset: false,
                tool_calls: vec![rumu_core::ToolCall {
                    call_id: "c1".into(),
                    name: "bash".into(),
                    args: json!({ "command": command }),
                }],
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
        .map(|i| rumu_core::ToolCall {
            call_id: format!("c{i}"),
            name: "bash".into(),
            args: json!({ "command": "y".repeat(280) }),
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
                reason_ms: None,
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
                content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
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
                reason_ms: None,
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
                        content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
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
                        content: rumu_core::Content::marker(&rumu_core::SessionRoot {}),
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
                        reason_ms: None,
                        reasoning_reset: false,
                        tool_calls: vec![rumu_core::ToolCall {
                            call_id: "c1".into(),
                            name: "bash".into(),
                            args: json!({ "command": "cat ~/.ssh/config" }),
                        }],
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
    std::fs::create_dir_all(home.path().join("cards")).unwrap();
    std::fs::write(
        home.path().join("cards/gateway.md"),
        "---\ntitle: Gateway\n---\nthe example server runs nginx and sshd\n",
    )
    .unwrap();
    let plugin = WorkspacePlugin::new(home.path(), WorkspaceCatalog::default());

    // Default scope = cards; hits are `@rumu:/cards/<id>` addresses.
    let out = plugin
        .call_tool("search", json!({ "query": "gateway" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("@rumu:/cards/gateway"),
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
    let lines: Vec<&str> = out.content.text.lines().collect();
    let addrs: Vec<&str> = lines
        .iter()
        .filter(|l| l.starts_with("@rumu:/sessions/"))
        .map(|l| l.split("  ").next().unwrap())
        .collect();
    assert_eq!(
        addrs,
        vec![
            format!("@rumu:/sessions/{s1}/N1").as_str(),
            format!("@rumu:/sessions/{s1}/U1").as_str(),
            format!("@rumu:/sessions/{s2}/U2").as_str(),
            format!("@rumu:/sessions/{s1}/A1").as_str(),
            format!("@rumu:/sessions/{s1}/T1").as_str(),
        ],
        "{}",
        out.content.text
    );
    assert!(
        lines.iter().any(|l| l.starts_with("  match: ")),
        "snippets: {}",
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
    let addrs: Vec<&str> = out
        .content
        .text
        .lines()
        .filter(|l| l.starts_with("@rumu:/sessions/"))
        .map(|l| l.split("  ").next().unwrap())
        .collect();
    assert_eq!(
        addrs.first().copied(),
        Some(format!("@rumu:/sessions/{s1}/U1").as_str()),
        "{}",
        out.content.text
    );
    assert!(
        addrs.contains(&format!("@rumu:/sessions/{s2}/U2").as_str()),
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
        out.content
            .text
            .contains(&format!("@rumu:/sessions/{s1}/A1")),
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
        out.content
            .text
            .contains(&format!("@rumu:/sessions/{s2}/U2"))
            && !out.content.text.contains(&format!("@rumu:/sessions/{s1}/")),
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

/// Equal text scores equally; a tool result is halved, and a score tie is
/// broken newest first by node time.
#[tokio::test]
async fn search_downweights_tool_results_and_breaks_ties_by_time() {
    let host = FakeRuntime::arc();
    let sid = rumu_core::SessionId::from("01SSESSIONTOOLS00000000000000");
    let info = rumu_core::SessionInfo {
        id: sid.clone(),
        title: "widgets".into(),
        active_branch: rumu_core::BranchId::from("B"),
        created_at: 1,
        updated_at: 1,
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
                "TOOL",
                300,
                NodeBody::ToolResult {
                    call_id: "c1".into(),
                    content: rumu_core::Content::text("widget"),
                    is_error: false,
                },
            ),
        ],
        vec![],
    ));
    let home = tempfile::tempdir().unwrap();
    let plugin = WorkspacePlugin::new(home.path(), WorkspaceCatalog::default());
    let cx = host.cx();
    let out = plugin
        .call_tool(
            "search",
            json!({ "query": "widget", "scope": "@rumu:/sessions" }),
            &cx,
        )
        .await
        .unwrap();
    let addrs: Vec<&str> = out
        .content
        .text
        .lines()
        .filter(|l| l.starts_with("@rumu:/sessions/"))
        .map(|l| l.split("  ").next().unwrap())
        .collect();
    assert_eq!(
        addrs,
        vec![
            format!("@rumu:/sessions/{sid}/UNEW").as_str(),
            format!("@rumu:/sessions/{sid}/UOLD").as_str(),
            format!("@rumu:/sessions/{sid}/TOOL").as_str(),
        ],
        "{}",
        out.content.text
    );
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

/// A background bash future that is dropped kills its process (`kill_on_drop`).
#[tokio::test]
async fn background_bash_is_killed_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let (plugin, host, _cx, _home) = setup(dir.path(), false);
    let cx = host.cx_with(true);
    let script = format!("echo $$ > {}; exec sleep 60", pid_file.display());
    let args = json!({ "command": script });

    let handle = tokio::spawn(async move {
        let _ = plugin.call_tool("bash", args, &cx).await;
    });
    // Wait until the shell wrote its pid, then drop the future.
    let mut pid = None;
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(&pid_file)
            && let Ok(p) = text.trim().parse::<i32>()
        {
            pid = Some(p);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let pid = pid.expect("bash started");
    handle.abort();
    let _ = handle.await;

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

/// `read @rumu:/task/<id>` renders the task's status and buffered output.
#[tokio::test]
async fn task_read_renders_status_and_output() {
    use rumu_core::{Content, TaskId, TaskInfo, TaskKind, TaskStatus};
    let home = tempfile::tempdir().unwrap();
    let plugin = WorkspacePlugin::new(home.path(), WorkspaceCatalog::default());
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
        .call_tool("read", json!({ "path": "@rumu:/task/t1" }), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(out.content.text.contains("task t1 · ok · bash: ls"));
    assert!(out.content.text.contains("partial\nmore output"));
    assert!(out.content.text.contains("--- result ---\nignored"));

    let listing = plugin
        .call_tool("read", json!({ "path": "@rumu:/task" }), &cx)
        .await
        .unwrap();
    assert!(!listing.is_error, "{}", listing.content.text);
    assert!(listing.content.text.contains("@rumu:/task/t1"));
}
