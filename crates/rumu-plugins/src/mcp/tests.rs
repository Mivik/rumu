use std::collections::BTreeMap;
use std::sync::Arc;

use rumu_core::{McpServerSettings, McpStatus, Plugin, Utf8UnixPath};
use serde_json::json;

use super::*;
use crate::testutil::FakeRuntime;

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

fn state(servers: &[&str]) -> PathState {
    PathState {
        mcp: all(servers),
        ..Default::default()
    }
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
    let st = state(&["fake", "broken", "gone"]);
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
    let other = state(&["broken"]);
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
    let plugin = McpPlugin::new(home.path(), hub);
    let fake = FakeRuntime::arc();
    fake.state.lock().unwrap().mcp = all(&["fake"]);
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

#[tokio::test]
async fn intro_note_lists_the_servers_once() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let plugin = McpPlugin::new(home.path(), hub);
    let fake = FakeRuntime::arc();
    let event = rumu_core::Event {
        kind: "llm.pre".into(),
        session: Default::default(),
        branch: Default::default(),
        fields: Default::default(),
        payload: serde_json::Value::Null,
    };
    let none = plugin
        .on_hook(INTRO_HOOK, &event, &fake.cx())
        .await
        .unwrap();
    assert!(none.effects.is_empty());

    fake.state.lock().unwrap().mcp = all(&["fake", "broken"]);
    let out = plugin
        .on_hook(INTRO_HOOK, &event, &fake.cx())
        .await
        .unwrap();
    let text = &out.effects[0].text;
    assert!(text.contains("- `fake`: 3 tools\n  Echo things."), "{text}");
    assert!(text.contains("- `broken`: unavailable ("), "{text}");
}

#[tokio::test]
async fn rules_narrow_a_servers_tools() {
    let home = tempfile::tempdir().unwrap();
    let Some(hub) = hub(home.path()).await else {
        return;
    };
    let rules = |r: &[&str]| PathState {
        mcp: [("fake".to_owned(), r.iter().map(|s| s.to_string()).collect())].into(),
        ..Default::default()
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
    let plugin = McpPlugin::new(home.path(), hub);
    let fake = FakeRuntime::arc();
    fake.state.lock().unwrap().mcp = st.mcp.clone();
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
