//! `code` plugin tests: script semantics, output format, limits and the
//! async/thread bridge (against the in-memory `FakeRuntime`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use rumu_core::{Plugin, ToolOutput, ToolSpec};
use serde_json::json;

use super::*;
use crate::testutil::{FakeRuntime, ScriptedInner};

fn tool(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.into(),
        description: format!("the {name} tool."),
        schema: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
        background: false,
    }
}

fn plugin(names: &[&str]) -> CodePlugin {
    let specs: Vec<ToolSpec> = names.iter().map(|n| tool(n)).collect();
    CodePlugin::new(&specs)
}

async fn run_with(
    plugin: &CodePlugin,
    code: &str,
    timeout: Option<u64>,
) -> (ToolOutput, Arc<FakeRuntime>) {
    let host = FakeRuntime::arc();
    let cx = host.cx();
    let mut args = json!({ "code": code });
    if let Some(t) = timeout {
        args["timeout"] = json!(t);
    }
    let out = plugin.call_tool("code", args, &cx).await.unwrap();
    (out, host)
}

async fn run(plugin: &CodePlugin, code: &str) -> ToolOutput {
    run_with(plugin, code, None).await.0
}

fn queue(host: &Arc<FakeRuntime>, specs: Vec<ScriptedInner>) {
    host.inner_specs.lock().unwrap().extend(specs);
}

#[tokio::test]
async fn result_shapes_and_console() {
    let p = plugin(&["read"]);

    let out = run(
        &p,
        r#"console.log("x", 1, {a: 2}); console.error("e"); return "hi";"#,
    )
    .await;
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.starts_with("[code] ok · 0 calls · "),
        "{}",
        out.content.text
    );
    assert!(
        out.content
            .text
            .contains("--- console ---\nx 1 {\"a\":2}\ne\n"),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.ends_with("--- result ---\nhi"),
        "{}",
        out.content.text
    );

    let out = run(&p, r#"return {b: 2, a: 1};"#).await;
    assert!(
        out.content
            .text
            .contains("--- result ---\n{\n  \"b\": 2,\n  \"a\": 1\n}"),
        "{}",
        out.content.text
    );

    let out = run(&p, r#"const x = 1; return;"#).await;
    assert!(!out.is_error);
    assert!(
        !out.content.text.contains("--- result ---"),
        "{}",
        out.content.text
    );
    assert!(
        !out.content.text.contains("--- console ---"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn tool_errors_and_blocked_errors() {
    let p = plugin(&["read"]);
    let code = r#"try { await rumu.read({path: "a"}); } catch (e) {
        return `${e instanceof Error}:${e instanceof ToolError}:${e.name}:${e.message}:${e.tool}`;
    }"#;

    // A plain tool failure → ToolError (and it is an Error).
    let host = FakeRuntime::arc();
    queue(&host, vec![ScriptedInner::err("kaput")]);
    let out = p
        .call_tool("code", json!({ "code": code }), &host.cx())
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .ends_with("--- result ---\ntrue:true:ToolError:kaput:read"),
        "{}",
        out.content.text
    );

    // A hook block → BlockedError, a ToolError subclass, message = reason.
    let host = FakeRuntime::arc();
    queue(&host, vec![ScriptedInner::blocked("nope")]);
    let out = p
        .call_tool("code", json!({ "code": code }), &host.cx())
        .await
        .unwrap();
    assert!(!out.is_error);
    assert!(
        out.content
            .text
            .ends_with("--- result ---\ntrue:true:BlockedError:nope:read"),
        "{}",
        out.content.text
    );
    assert!(
        out.content
            .text
            .starts_with("[code] ok · 1 call (1 blocked) · "),
        "{}",
        out.content.text
    );

    // Uncaught: is_error and a clean error section.
    let host = FakeRuntime::arc();
    queue(&host, vec![ScriptedInner::err("kaput")]);
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"await rumu.read({path: "a"});"# }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(
        out.content.text.contains("[code] error · 1 call · "),
        "{}",
        out.content.text
    );
    assert!(
        out.content.text.contains("--- error ---\nToolError: kaput"),
        "{}",
        out.content.text
    );
}

/// Internal image reads reach the script as their placeholder and forward
/// onto the `code` result, deduped by hash.
#[tokio::test]
async fn internal_images_forward_to_the_code_result() {
    let p = plugin(&["read"]);
    let image = |hash: char, name: &str| rumu_core::Attachment {
        hash: hash.to_string().repeat(64),
        mime: "image/png".into(),
        name: Some(name.into()),
        width: 800,
        height: 600,
        size: 1234,
    };
    let header = "image proj:a.png (800×600, 1234 bytes stored)";
    let host = FakeRuntime::arc();
    queue(
        &host,
        vec![
            ScriptedInner::ok(header).with_attachments(vec![image('a', "shot.png")]),
            // A second read adds one new blob; the first one is not repeated.
            ScriptedInner::ok(header)
                .with_attachments(vec![image('a', "shot.png"), image('b', "other.png")]),
        ],
    );
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"const a = await rumu.read({path: "a.png"});
                await rumu.read({path: "a.png"});
                return a.split("\n").pop();"# }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    // The script saw the placeholder, not the raw header line.
    assert!(
        out.content
            .text
            .ends_with("--- result ---\n[image: shot.png 800×600]"),
        "{}",
        out.content.text
    );
    let hashes: Vec<&str> = out
        .content
        .attachments
        .iter()
        .map(|a| a.hash.as_str())
        .collect();
    assert_eq!(hashes, ["a".repeat(64).as_str(), "b".repeat(64).as_str()]);
}

#[tokio::test]
async fn timeout_interrupts_infinite_loops() {
    let p = plugin(&[]);
    let started = Instant::now();
    let (out, _) = run_with(&p, "while (true) {}", Some(1)).await;
    assert!(out.is_error);
    assert!(
        out.content.text.starts_with("[code] timeout after 1 s"),
        "{}",
        out.content.text
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the interrupt fired promptly"
    );
}

#[tokio::test]
async fn memory_limit_is_a_clean_error() {
    let p = plugin(&[]);
    let (out, _) = run_with(
        &p,
        r#"const a = []; while (true) { a.push("x".repeat(1024 * 1024)); }"#,
        Some(10),
    )
    .await;
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("--- error ---"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn call_limit_is_enforced() {
    let p = plugin(&["read"]);
    let host = FakeRuntime::arc();
    queue(&host, (0..205).map(|_| ScriptedInner::ok("x")).collect());
    let cx = host.cx();
    let out = p
        .call_tool(
            "code",
            json!({ "code": "for (let i = 0; i < 205; i++) { await rumu.read({path: 'a'}); }" }),
            &cx,
        )
        .await
        .unwrap();
    assert!(out.is_error, "{}", out.content.text);
    assert!(
        out.content.text.contains("too many tool calls (limit 200)"),
        "{}",
        out.content.text
    );
    assert_eq!(host.inner_calls.lock().unwrap().len(), 200);
}

#[tokio::test]
async fn weird_names_use_call_and_code_is_not_available() {
    let p = plugin(&["read", "weird-name"]);
    let host = FakeRuntime::arc();
    queue(&host, vec![ScriptedInner::ok("w")]);
    let cx = host.cx();
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"const a = await rumu.call("weird-name", {});
                return a + ":" + typeof rumu["weird-name"] + ":" + typeof rumu.code;"# }),
            &cx,
        )
        .await
        .unwrap();
    assert!(
        out.content
            .text
            .ends_with("--- result ---\nw:undefined:undefined"),
        "{}",
        out.content.text
    );
    let calls = host.inner_calls.lock().unwrap();
    assert_eq!(calls[0].2, "weird-name");
    assert_eq!(calls[0].1, "call-1", "the enclosing code call id");
}

#[tokio::test]
async fn native_bridge_is_not_reachable() {
    let p = plugin(&["read"]);
    let out = run(
        &p,
        "return [typeof __call, typeof globalThis.__api, typeof globalThis.__call].join(',');",
    )
    .await;
    assert!(
        out.content
            .text
            .ends_with("--- result ---\nundefined,undefined,undefined"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn promise_that_never_settles_is_reported() {
    let p = plugin(&[]);
    let out = run_with(&p, "return new Promise(() => {});", Some(5))
        .await
        .0;
    assert!(out.is_error);
    assert!(
        out.content.text.contains("Error: script did not settle"),
        "{}",
        out.content.text
    );
}

#[tokio::test]
async fn manifest_is_deterministic_and_excludes_denied_tools() {
    let a = plugin(&["read", "bash", "code", "agent"]);
    let b = plugin(&["agent", "bash", "code", "read"]);
    assert_eq!(
        a.manifest().tools[0].description,
        b.manifest().tools[0].description
    );
    let desc = &a.manifest().tools[0].description;
    assert!(desc.contains("rumu.read("));
    assert!(desc.contains("rumu.bash("));
    assert!(desc.contains("rumu.agent("), "agent is script-callable");
    assert!(!desc.contains("rumu.code("), "only `code` is denied");
    assert_eq!(a.manifest().name, "code");
}

#[tokio::test]
async fn missing_code_arg_is_an_error() {
    let p = plugin(&[]);
    let host = FakeRuntime::arc();
    let out = p.call_tool("code", json!({}), &host.cx()).await.unwrap();
    assert!(out.is_error);
    assert!(
        out.content
            .text
            .contains("code: invalid arguments: missing field `code`")
    );
}
