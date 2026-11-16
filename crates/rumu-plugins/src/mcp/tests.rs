use std::collections::BTreeMap;
use std::sync::Arc;

use rumu_core::{McpServerSettings, McpStatus, Plugin, Utf8UnixPath};
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

fn path(p: &str) -> &Utf8UnixPath {
    Utf8UnixPath::new(p)
}

#[tokio::test]
async fn connects_and_reports_status() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let fake = hub.server("fake").unwrap().snapshot();
    assert_eq!(fake.status, McpStatus::Ready);
    assert_eq!(fake.tools.len(), 3);
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
    let st = all(&["fake", "broken", "gone"]);
    let root = list(&hub, &st, path("/tools")).await.unwrap().content.text;
    assert!(root.contains("fake/ — 3 tools"), "{root}");
    assert!(root.contains("broken/ — unavailable"), "{root}");
    assert!(root.contains("gone/ — not configured"), "{root}");

    let tools = list(&hub, &st, path("/tools/fake"))
        .await
        .unwrap()
        .content
        .text;
    assert!(tools.contains("list_issues — List the open issues of a GitHub repository."));
    assert!(!tools.contains("Paged by 30"));

    let tool = read(&hub, &st, path("/tools/fake/echo"))
        .await
        .unwrap()
        .content
        .text;
    assert!(
        tool.starts_with("## fake/echo\nEcho the given text back.\nInput schema: {"),
        "{tool}"
    );

    let hits = search(&hub, &st, path("/tools"), "github issues", 5)
        .await
        .unwrap()
        .content
        .text;
    assert!(
        hits.starts_with("## fake/list_issues\nList issues\n"),
        "{hits}"
    );
    assert!(!hits.contains("fake/echo"));

    // A server the session does not have is neither listed nor readable.
    let other = all(&["broken"]);
    let err = read(&hub, &other, path("/tools/fake/echo"))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no MCP server `fake` in this session"),
        "{err}"
    );
}

#[tokio::test]
async fn call_tool_returns_text_errors_and_structured_content() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let decks = assistant_decks(home.path());
    set_assistant(&decks, all(&["fake"]), &[]);
    let plugin = McpPlugin::new(home.path(), hub, decks);
    let fake = FakeRuntime::arc();
    let cx = fake.cx();
    let call = |args| plugin.call_tool("call_tool", args, &cx);

    let out = call(json!({ "tool": "fake/echo", "args": { "text": "hi" } }))
        .await
        .unwrap();
    assert!(!out.is_error);
    assert_eq!(out.content.text, "hi");

    let out = call(json!({ "tool": "fake/fail" })).await.unwrap();
    assert!(out.is_error);
    assert_eq!(out.content.text, "boom");

    let out = call(json!({ "tool": "fake/list_issues" })).await.unwrap();
    assert!(
        out.content.text.contains("\"issues\""),
        "{}",
        out.content.text
    );

    let out = call(json!({ "tool": "fake/nope" })).await.unwrap();
    assert!(out.is_error);
    assert!(
        out.content
            .text
            .contains("its tools: `echo`, `list_issues`, `fail`")
    );

    let out = call(json!({ "tool": "broken/x" })).await.unwrap();
    assert!(out.is_error && out.content.text.contains("no MCP server `broken`"));
}

/// An image block is attached and copied into `@scratch:.inbox/attachments/`,
/// the text naming the file; without a scratch root it is only attached.
#[tokio::test]
async fn an_image_result_lands_in_the_scratch_inbox() {
    const DOT_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let home = tempfile::tempdir().unwrap();
    let decks = assistant_decks(home.path());
    let plugin = McpPlugin::new(home.path(), McpHub::new(home.path()), decks);
    let result = json!({ "content": [
        { "type": "text", "text": "a dot" },
        { "type": "image", "mimeType": "image/png", "data": DOT_PNG },
    ] });
    let scratch = home.path().join("scratch/s1");

    let out = plugin.output(&result, Some(scratch.clone())).await.unwrap();
    let [image] = &out.content.attachments[..] else {
        panic!("one attachment: {:?}", out.content.attachments)
    };
    let file = image.scratch_path();
    assert_eq!(
        out.content.text,
        format!("a dot\n[image 1 saved as @scratch:{file}]")
    );
    assert!(scratch.join(&file).is_file());

    let out = plugin.output(&result, None).await.unwrap();
    assert_eq!(out.content.text, "a dot");
    assert_eq!(out.content.attachments.len(), 1);
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
    let plugin = McpPlugin::new(home.path(), hub, decks.clone());
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
    let st = rules(&["*", "!fail"]);
    assert!(text(list(&hub, &st, path("/tools")).await).contains("fake/ — 2 tools"));
    let tools = text(list(&hub, &st, path("/tools/fake")).await);
    assert!(tools.contains("echo") && !tools.contains("fail"), "{tools}");
    let err = read(&hub, &st, path("/tools/fake/fail")).await.unwrap_err();
    assert!(
        err.to_string().contains("its tools: `echo`, `list_issues`"),
        "{err}"
    );
    let hits = text(search(&hub, &st, path("/tools"), "always fails", 5).await);
    assert!(!hits.contains("fake/fail"), "{hits}");

    // Only the one named.
    let st = rules(&["echo"]);
    assert_eq!(
        text(list(&hub, &st, path("/tools/fake")).await),
        "echo — Echo the given text back.\n"
    );
    let decks = assistant_decks(home.path());
    set_assistant(&decks, st, &[]);
    let plugin = McpPlugin::new(home.path(), hub, decks);
    let fake = FakeRuntime::arc();
    let out = plugin
        .call_tool(
            "call_tool",
            json!({ "tool": "fake/list_issues" }),
            &fake.cx(),
        )
        .await
        .unwrap();
    assert!(out.is_error && out.content.text.contains("has no tool `list_issues`"));
}
