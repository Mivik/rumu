use std::collections::BTreeMap;
use std::sync::Arc;

use rumu_core::{McpServerSettings, McpStatus, Plugin};
use serde_json::json;

use super::*;
use crate::testutil::{FakeRuntime, assistant_decks, set_assistant};

const SERVER: &str = include_str!("fake_server.py");

/// A hub with the fake server as `fake` and a missing program as `broken`,
/// both connected; `None` without python3.
async fn hub(home: &std::path::Path) -> Option<Arc<McpHub>> {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("python3 not found; skipping");
        return None;
    }
    let script = home.join("fake_server.py");
    std::fs::write(&script, SERVER).unwrap();
    let hub = McpHub::new(home);
    let mut settings = BTreeMap::new();
    settings.insert(
        "fake".to_owned(),
        McpServerSettings {
            command: vec!["python3".into(), script.display().to_string()],
            ..Default::default()
        },
    );
    settings.insert(
        "broken".to_owned(),
        McpServerSettings {
            command: vec!["rumu-no-such-mcp-server".into()],
            ..Default::default()
        },
    );
    hub.configure(&settings);
    for name in ["fake", "broken"] {
        hub.server(name).unwrap().ready().await;
    }
    Some(hub)
}

/// Each server with all its tools.
fn all(servers: &[&str]) -> rumu_core::McpUse {
    servers
        .iter()
        .map(|s| (s.to_string(), vec!["*".to_owned()]))
        .collect()
}

/// `mcp` as a session's servers when one deck binds them.
fn one_deck(mcp: rumu_core::McpUse) -> Servers {
    mcp.into_iter()
        .map(|(name, rules)| (name, vec![rules]))
        .collect()
}

#[tokio::test]
async fn connects_and_reports_status() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let fake = hub.server("fake").unwrap().snapshot();
    assert_eq!(fake.status, McpStatus::Ready);
    // `poll` is only for views: the model sees three.
    assert_eq!(fake.tools.len(), 4);
    assert_eq!(fake.instructions.as_deref(), Some("Echo things."));
    let broken = hub.server("broken").unwrap().snapshot();
    assert_eq!(broken.status, McpStatus::Failed);
    assert!(broken.error.unwrap().contains("rumu-no-such-mcp-server"));
}

#[tokio::test]
async fn lists_reads_and_searches_the_bound_servers() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let st = one_deck(all(&["fake", "broken", "gone"]));
    let root = list_servers(&hub, &st).await.content.text;
    assert!(root.contains("fake/ — 3 tools"), "{root}");
    assert!(root.contains("broken/ — unavailable"), "{root}");
    assert!(root.contains("gone/ — not configured"), "{root}");

    // A server lists its resources, then its tools as its actions.
    let tools = list_server(&hub, &st, "fake", [])
        .await
        .unwrap()
        .content
        .text;
    assert!(tools.starts_with("MCP server `fake`: 3 tools;"), "{tools}");
    assert!(
        tools.contains(
            "\n\nresources:\n  mem://notes — notes: Meeting notes. (text/markdown)\n  \
             mem://both — both\n\nresource templates (fill in the URI to read):\n  \
             mem://issue/{n} — issue: One issue.\n\n"
        ),
        "{tools}"
    );
    assert!(
        tools.contains("\n\nactions:\n  fake/echo(text: string) — Echo the given text back.\n"),
        "{tools}"
    );
    assert!(tools.contains("list_issues"), "{tools}");
    assert!(!tools.contains("Paged by 30"));

    // Reading it gives its instructions, every tool's input schema and its
    // resources.
    let server = read_server(&hub, &st, "fake").await.unwrap().content.text;
    assert!(
        server.starts_with("MCP server `fake`: 3 tools\n\nEcho things.\n"),
        "{server}"
    );
    assert!(
        server.contains("action fake/echo\nEcho the given text back.\nInput schema: {"),
        "{server}"
    );
    assert!(server.contains("\nresources:\n  mem://notes"), "{server}");

    let hits = search(&hub, &st, None, "github issues", 5)
        .await
        .unwrap()
        .content
        .text;
    assert!(
        hits.starts_with("action fake/list_issues\nList issues\n"),
        "{hits}"
    );
    assert!(!hits.contains("fake/echo"));

    // A server the session does not have is neither listed nor readable.
    let other = one_deck(all(&["broken"]));
    let err = read_server(&hub, &other, "fake").await.unwrap_err();
    assert!(
        err.to_string()
            .contains("no MCP server `fake` in this session"),
        "{err}"
    );
}

/// The workspace plugin's `invoke` with `<server>/<tool>`.
fn invoker(home: &std::path::Path, hub: Arc<McpHub>, decks: Decks) -> crate::WorkspacePlugin {
    crate::WorkspacePlugin::new(home, decks, Default::default()).with_mcp(hub)
}

#[tokio::test]
async fn invoke_returns_text_errors_and_structured_content() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    set_assistant(&decks, all(&["fake"]), &[]);
    let plugin = invoker(home.path(), hub, decks);
    let fake = FakeRuntime::arc();
    let cx = fake.cx();
    let call = |args| plugin.call_tool("invoke", args, &cx);

    let out = call(json!({ "action": "fake/echo", "args": { "text": "hi" } }))
        .await
        .unwrap();
    assert!(!out.is_error);
    assert_eq!(out.content.text, "hi");

    let out = call(json!({ "action": "fake/fail" })).await.unwrap();
    assert!(out.is_error);
    assert_eq!(out.content.text, "boom");

    let out = call(json!({ "action": "fake/list_issues" })).await.unwrap();
    assert!(
        out.content.text.contains("\"issues\""),
        "{}",
        out.content.text
    );

    let out = call(json!({ "action": "fake/nope" })).await.unwrap();
    assert!(out.is_error);
    assert!(
        out.content
            .text
            .contains("its tools: `echo`, `list_issues`, `fail`")
    );

    let out = call(json!({ "action": "broken/x" })).await.unwrap();
    assert!(out.is_error && out.content.text.contains("no MCP server `broken`"));

    // An MCP tool is its server's action: it takes no `path`.
    let out = call(json!({ "action": "fake/echo", "path": "@rumu:/mcp/fake" }))
        .await
        .unwrap();
    assert!(
        out.is_error
            && out
                .content
                .text
                .contains("is an MCP tool: it takes no `path`"),
        "{}",
        out.content.text
    );
}

/// `mcp.spawn` opens a session's own connection (a new process), used like
/// its server, seen only by that session, closed by `delete`.
#[tokio::test]
async fn a_session_spawns_and_closes_its_own_instance() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    set_assistant(&decks, all(&["fake"]), &[]);
    let plugin = invoker(home.path(), hub.clone(), decks);
    let fake = FakeRuntime::arc();
    let (cx, other) = (fake.cx_for("s1".into()), fake.cx_for("s2".into()));
    let starts = || {
        std::fs::read_to_string(home.path().join("starts"))
            .unwrap()
            .lines()
            .count()
    };
    assert_eq!(starts(), 1);

    let spawn = json!({ "action": "mcp.spawn", "path": "@rumu:/mcp/fake" });
    let out = plugin
        .call_tool("invoke", spawn.clone(), &cx)
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let name = out.content.text.split('`').nth(1).unwrap().to_owned();
    assert!(name.starts_with("fake#"), "{name}");
    assert_eq!(starts(), 2);

    let echo = json!({ "action": format!("{name}/echo"), "args": { "text": "hi" } });
    let out = plugin.call_tool("invoke", echo.clone(), &cx).await.unwrap();
    assert_eq!(out.content.text, "hi");
    let list = json!({ "path": "@rumu:/mcp" });
    let root = plugin.call_tool("list", list.clone(), &cx).await.unwrap();
    assert!(
        root.content
            .text
            .contains(&format!("{name}/ — 3 tools, an instance of `fake`")),
        "{}",
        root.content.text
    );
    // Another session neither sees nor reaches it.
    let root = plugin.call_tool("list", list, &other).await.unwrap();
    assert!(!root.content.text.contains(&name));
    let out = plugin
        .call_tool("invoke", echo.clone(), &other)
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("has no instance"));

    // Nor an instance of an instance; and a configured server stays.
    let again = json!({ "action": "mcp.spawn", "path": format!("@rumu:/mcp/{name}") });
    let out = plugin.call_tool("invoke", again, &cx).await.unwrap();
    assert!(out.is_error && out.content.text.contains("already an instance"));
    let out = plugin
        .call_tool("delete", json!({ "path": "@rumu:/mcp/fake" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("configured server"));

    let path = json!({ "path": format!("@rumu:/mcp/{name}") });
    let out = plugin
        .call_tool("delete", path.clone(), &other)
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    let out = plugin.call_tool("delete", path, &cx).await.unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    let out = plugin.call_tool("invoke", echo, &cx).await.unwrap();
    assert!(out.is_error && out.content.text.contains("mcp.spawn"));

    // A session opens at most so many.
    for _ in 0..hub::INSTANCES_PER_SESSION {
        let out = plugin
            .call_tool("invoke", spawn.clone(), &cx)
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content.text);
    }
    let out = plugin.call_tool("invoke", spawn, &cx).await.unwrap();
    assert!(out.is_error && out.content.text.contains("already has"));
}

/// A server on MCP 2026-07-28 keeps no state per connection: spawning one
/// is refused, unless forced for a stdio server.
#[tokio::test]
async fn spawning_a_stateless_server_needs_force() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let mut settings = BTreeMap::new();
    settings.insert(
        "modern".to_owned(),
        McpServerSettings {
            command: vec![
                "python3".into(),
                home.path().join("fake_server.py").display().to_string(),
            ],
            env: [("FAKE_MODERN".to_owned(), "1".to_owned())].into(),
            ..Default::default()
        },
    );
    hub.configure(&settings);
    let modern = hub.server("modern").unwrap();
    assert_eq!(modern.ready().await.status, McpStatus::Ready);
    assert!(modern.stateless());

    let st = one_deck(all(&["modern"]));
    let session = "s1".into();
    let err = spawn(&hub, &st, &session, "modern", false)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("speaks MCP 2026-07-28"), "{err}");
    assert!(err.contains("`force: true`"), "{err}");
    let out = text_of(spawn(&hub, &st, &session, "modern", true).await);
    assert!(out.starts_with("Opened `modern#"), "{out}");
}

/// An image block is attached and copied into `@scratch:.inbox/attachments/`,
/// the text naming the file; without a scratch root it is only attached.
#[tokio::test]
async fn an_image_result_lands_in_the_scratch_inbox() {
    const DOT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let home = tempfile::tempdir().unwrap();
    let blobs = || rumu_blob::Blobs::in_home(home.path());
    let result = json!({ "content": [
        { "type": "text", "text": "a dot" },
        { "type": "image", "mimeType": "image/png", "data": DOT_PNG },
    ] });
    let scratch = home.path().join("scratch/s1");

    let out = output(&result, "fake", Some(scratch.clone()), blobs)
        .await
        .unwrap();
    let [image] = &out.content.attachments[..] else {
        panic!("one attachment: {:?}", out.content.attachments)
    };
    let file = image.scratch_path();
    assert_eq!(
        out.content.text,
        format!("a dot\n[image 1 saved as @scratch:{file}]")
    );
    assert!(scratch.join(&file).is_file());

    let out = output(&result, "fake", None, blobs).await.unwrap();
    assert_eq!(out.content.text, "a dot");
    assert_eq!(out.content.attachments.len(), 1);
}

/// Other media land in `@scratch` as they are, under a short extension; a
/// resource link says how to read it.
#[tokio::test]
async fn other_media_land_in_the_scratch_inbox_and_links_say_how_to_read() {
    let home = tempfile::tempdir().unwrap();
    let blobs = || rumu_blob::Blobs::in_home(home.path());
    let result = json!({ "content": [
        { "type": "audio", "mimeType": "audio/wav", "data": "UklGRg==" },
        { "type": "resource", "resource": {
            "uri": "mem://a.bin", "mimeType": "../../x", "blob": "aGkK" } },
        { "type": "resource_link", "uri": "mem://notes", "name": "notes" },
    ] });
    let scratch = home.path().join("scratch/s1");

    let out = output(&result, "fake", Some(scratch.clone()), blobs)
        .await
        .unwrap();
    let wav = rumu_core::scratch_attachment(&rumu_blob::hash_of(b"RIFF"), "audio/wav");
    let bin = rumu_core::scratch_attachment(&rumu_blob::hash_of(b"hi\n"), "../../x");
    assert!(wav.ends_with(".wav") && bin.ends_with(".bin"));
    assert_eq!(
        out.content.text,
        format!(
            "[audio (audio/wav) saved as @scratch:{wav}]\n\
             [resource mem://a.bin (../../x) saved as @scratch:{bin}]\n\
             [resource link: mem://notes (notes); read it with `invoke` `mcp.read_resource` \
             on `@rumu:/mcp/fake`]"
        )
    );
    assert_eq!(std::fs::read(scratch.join(&bin)).unwrap(), b"hi\n");
    assert!(out.content.attachments.is_empty());

    let out = output(&result, "fake", None, blobs).await.unwrap();
    assert!(
        out.content
            .text
            .starts_with("[audio (audio/wav) omitted]\n[resource mem://a.bin (../../x) omitted]"),
        "{}",
        out.content.text
    );
}

/// `mcp.read_resource` on a server's path reads one of its resources.
#[tokio::test]
async fn invoke_reads_a_resource() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    set_assistant(&decks, all(&["fake"]), &[]);
    let plugin = invoker(home.path(), hub, decks);
    let fake = FakeRuntime::arc();
    let cx = fake.cx();
    let read = |path: &str, uri: &str| {
        let args = json!({ "action": "mcp.read_resource", "path": path, "args": { "uri": uri } });
        plugin.call_tool("invoke", args, &cx)
    };

    let out = read("@rumu:/mcp/fake", "mem://notes").await.unwrap();
    assert!(!out.is_error);
    assert_eq!(out.content.text, "# Notes");

    // Several contents come each under its URI; without a scratch root a
    // blob is only named.
    let out = read("@rumu:/mcp/fake", "mem://both").await.unwrap();
    assert_eq!(
        out.content.text,
        "[mem://both/a]\nA\n[mem://both/b]\n[resource mem://both/b (application/x-thing) omitted]"
    );

    let out = read("@rumu:/mcp/fake", "mem://nope").await.unwrap();
    assert!(
        out.is_error && out.content.text.contains("fake: reading `mem://nope`: "),
        "{}",
        out.content.text
    );

    let out = read("@rumu:/mcp/broken", "mem://notes").await.unwrap();
    assert!(out.is_error && out.content.text.contains("no MCP server `broken`"));

    // The action shows in the server's listing, after its tools.
    let out = plugin
        .call_tool("list", json!({ "path": "@rumu:/mcp/fake" }), &cx)
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .contains("\n  mcp.read_resource(uri: string) — read one of its resources (`resources/read`); media land in `@scratch`\n  mcp.spawn("),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn a_dropped_call_is_not_retried_and_reconnects_once() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let fake = hub.server("fake").unwrap();
    let log = home.path().join("ran");
    let mut args = JsonObject::new();
    args.insert("log".into(), json!(log.display().to_string()));
    assert!(fake.call("crash", args).await.is_err());
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "ran\n");

    let echo = || {
        let fake = fake.clone();
        async move {
            let mut args = JsonObject::new();
            args.insert("text".into(), json!("hi"));
            fake.call("echo", args).await
        }
    };
    let (a, b, c) = tokio::join!(echo(), echo(), echo());
    for r in [a, b, c] {
        assert_eq!(r.unwrap()["content"][0]["text"], "hi");
    }
    let starts = std::fs::read_to_string(home.path().join("starts")).unwrap();
    assert_eq!(starts.lines().count(), 2);
}

#[tokio::test]
async fn intro_note_follows_the_assistants_servers() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    let plugin = McpPlugin::new(hub, decks.clone());
    let fake = FakeRuntime::arc();
    let event = rumu_core::Event {
        kind: "llm.pre".into(),
        session: Default::default(),
        branch: Default::default(),
        fields: Default::default(),
        payload: serde_json::Value::Null,
    };
    let hook = || async {
        let out = plugin
            .on_hook(INTRO_HOOK, &event, &fake.cx())
            .await
            .unwrap();
        fake.apply_hook("mcp", &out);
        out.effects.into_iter().map(|e| e.text).collect::<Vec<_>>()
    };
    assert!(hook().await.is_empty());

    set_assistant(&decks, all(&["fake", "broken"]), &[]);
    let text = hook().await.remove(0);
    assert!(text.starts_with("MCP servers of this session."), "{text}");
    assert!(text.contains("- `fake`: 3 tools\n  Echo things."), "{text}");
    assert!(text.contains("- `broken`: unavailable ("), "{text}");
    assert!(hook().await.is_empty());

    // Changing the assistant mid-session reaches the session.
    set_assistant(&decks, all(&["fake"]), &[]);
    let text = hook().await.remove(0);
    assert!(text.starts_with("The MCP servers changed;"), "{text}");
    assert!(!text.contains("broken"), "{text}");
    assert!(hook().await.is_empty());

    set_assistant(&decks, Default::default(), &[]);
    assert_eq!(hook().await, ["This session has no MCP servers any more."]);
    assert!(hook().await.is_empty());
}

#[tokio::test]
async fn a_project_adds_its_servers_and_tools() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let rules = |r: &[&str]| -> Vec<String> { r.iter().map(|s| s.to_string()).collect() };
    let decks = assistant_decks(home.path());
    set_assistant(
        &decks,
        [("fake".to_owned(), rules(&["*", "!list_issues"]))].into(),
        &[],
    );
    let change = crate::ProjectChange {
        name: Some("proj".into()),
        mcp: Some(
            [
                ("fake".to_owned(), rules(&["list_issues"])),
                ("broken".to_owned(), rules(&["*"])),
            ]
            .into(),
        ),
        ..Default::default()
    };
    let project = crate::create_project(home.path(), &decks, &Default::default(), change).unwrap();
    let mut state = PathState {
        assistant: crate::testutil::TEST_ASSISTANT.into(),
        ..Default::default()
    };
    let root = |state: &PathState| {
        let st = servers(&decks, &hub, &Default::default(), state);
        let hub = hub.clone();
        async move { list_servers(&hub, &st).await.content.text }
    };
    assert_eq!(root(&state).await, "fake/ — 2 tools\n");

    // In the project, its servers join the assistant's, and a tool either
    // enables is in: the assistant's exclusion does not take it away.
    state
        .mounts
        .push(rumu_core::CardMount::new(project.id(), "/cards/proj", true));
    let text = root(&state).await;
    assert!(text.contains("fake/ — 3 tools"), "{text}");
    assert!(text.contains("broken/ — unavailable"), "{text}");

    // A project deck only borrowed adds nothing.
    state.mounts[0].project = false;
    assert_eq!(root(&state).await, "fake/ — 2 tools\n");
}

#[tokio::test]
async fn rules_narrow_a_servers_tools() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let rules = |r: &[&str]| -> rumu_core::McpUse {
        [("fake".to_owned(), r.iter().map(|s| s.to_string()).collect())].into()
    };
    let text = |out: ToolResult<ToolOutput>| out.unwrap().content.text;

    // All but one: new tools would be in too.
    let st = one_deck(rules(&["*", "!fail"]));
    assert!(
        list_servers(&hub, &st)
            .await
            .content
            .text
            .contains("fake/ — 2 tools")
    );
    let tools = text(list_server(&hub, &st, "fake", []).await);
    assert!(
        tools.contains("fake/echo") && !tools.contains("fail"),
        "{tools}"
    );
    let server = text(read_server(&hub, &st, "fake").await);
    assert!(!server.contains("fake/fail"), "{server}");
    let hits = text(search(&hub, &st, None, "always fails", 5).await);
    assert!(!hits.contains("fake/fail"), "{hits}");

    // Only the one named.
    let st = rules(&["echo"]);
    assert!(
        text(list_server(&hub, &one_deck(st.clone()), "fake", []).await)
            .ends_with("actions:\n  fake/echo(text: string) — Echo the given text back."),
    );
    let decks = assistant_decks(home.path());
    set_assistant(&decks, st, &[]);
    let plugin = invoker(home.path(), hub, decks);
    let fake = FakeRuntime::arc();
    let out = plugin
        .call_tool(
            "invoke",
            json!({ "action": "fake/list_issues" }),
            &fake.cx(),
        )
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("has no tool `list_issues`"));
}

/// MCP Apps: a tool with a view has its page read along with the call and
/// recorded for the web; a tool only for views is out of the model's sight
/// but views reach it; what a view tells the model waits for its next call.
#[tokio::test]
async fn views_are_recorded_and_reach_their_servers_tools() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    set_assistant(&decks, all(&["fake"]), &[]);
    let plugin = invoker(home.path(), hub.clone(), decks.clone());
    let fake = FakeRuntime::arc();
    let cx = fake.cx();
    let args = json!({ "action": "fake/list_issues", "args": { "repo": "r" } });
    let out = plugin.call_tool("invoke", args, &cx).await.unwrap();
    assert!(
        out.content
            .text
            .ends_with("[The user sees this result in an interactive view.]")
    );
    let app: McpApp = out.content.data.unwrap().decode().unwrap();
    assert_eq!(
        (app.uri.as_str(), app.prefers_border),
        ("ui://fake/issues", Some(false))
    );
    // The origin that would add a directive is dropped.
    assert_eq!(app.csp.connect, ["https://api.github.com"]);
    let blobs = rumu_blob::Blobs::in_home(home.path()).unwrap();
    assert_eq!(
        blobs.get(&app.html).unwrap(),
        b"<!doctype html><p>issues</p>"
    );
    let call: Value = serde_json::from_slice(&blobs.get(&app.call).unwrap()).unwrap();
    assert_eq!(call["arguments"], json!({ "repo": "r" }));
    assert_eq!(
        call["result"]["structuredContent"],
        json!({ "issues": [1, 2] })
    );

    // `poll` is only for views.
    let st = one_deck(all(&["fake"]));
    assert!(!text_of(list_server(&hub, &st, "fake", []).await).contains("poll"));
    let out = plugin
        .call_tool("invoke", json!({ "action": "fake/poll" }), &cx)
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("has no tool `poll`"));
    let polled = app_call(&hub, &st, "fake", "poll", Default::default()).await;
    assert_eq!(polled.unwrap()["content"][0]["text"], "polled");
    // Views reach the tools of their server the rules enable, no others.
    let echoed = app_call(&hub, &st, "fake", "echo", Default::default()).await;
    assert!(echoed.is_ok());
    let narrow = one_deck([("fake".to_owned(), vec!["echo".to_owned()])].into());
    let refused = app_call(&hub, &narrow, "fake", "poll", Default::default()).await;
    assert!(refused.unwrap_err().contains("no tool `poll` for apps"));
    assert!(
        app_call(&hub, &st, "broken", "x", Default::default())
            .await
            .is_err()
    );
    let read = app_read(&hub, &st, "fake", "mem://notes").await.unwrap();
    assert_eq!(read["contents"][0]["text"], "# Notes");

    // Only a view's latest context reaches the next model call, once.
    let mcp = McpPlugin::new(hub.clone(), decks);
    let event = rumu_core::Event {
        kind: "llm.pre".into(),
        session: Default::default(),
        branch: cx.branch.clone(),
        fields: Default::default(),
        payload: Value::Null,
    };
    let hook = || async {
        let out = mcp.on_hook(INTRO_HOOK, &event, &cx).await.unwrap();
        fake.apply_hook("mcp", &out);
        let notes = out.effects.into_iter().map(|e| e.text);
        notes
            .filter(|t| t.starts_with("The user's"))
            .collect::<Vec<_>>()
    };
    let say = |text: &str| {
        let content = json!([{ "type": "text", "text": text }]);
        app_context(
            &hub,
            &cx.branch,
            "n1",
            ("fake", "list_issues"),
            &content,
            None,
        );
    };
    say("zoom 1");
    say("zoom 2");
    let notes = hook().await;
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].ends_with("replaces what it said before):\nzoom 2"),
        "{notes:?}"
    );
    assert!(hook().await.is_empty());
    // Saying nothing takes back what was not yet handed over.
    say("zoom 3");
    app_context(
        &hub,
        &cx.branch,
        "n1",
        ("fake", "list_issues"),
        &Value::Null,
        None,
    );
    assert!(hook().await.is_empty());
}

fn text_of(out: ToolResult<ToolOutput>) -> String {
    out.unwrap().content.text
}
