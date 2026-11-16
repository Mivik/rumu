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
        output_schema: None,
        parallel: false,
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

fn image(hash: char, name: &str) -> rumu_core::Attachment {
    rumu_core::Attachment {
        kind: rumu_core::AttachmentKind::Image,
        hash: hash.to_string().repeat(64),
        mime: "image/png".into(),
        name: Some(name.into()),
        width: 800,
        height: 600,
        size: 1234,
    }
}

/// A call that returned media resolves to `{ text, attachments }`; only
/// attachments passed to `attach()` reach the result, in output order, deduped by hash.
#[tokio::test]
async fn attachments_reach_the_result_only_through_attach() {
    let p = plugin(&["read"]);
    let header = "image proj:a.png (800×600, 1234 bytes stored)";
    let host = FakeRuntime::arc();
    queue(
        &host,
        vec![
            ScriptedInner::ok(header).with_attachments(vec![image('a', "shot.png")]),
            ScriptedInner::ok(header)
                .with_attachments(vec![image('b', "other.png"), image('c', "third.png")]),
        ],
    );
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"const a = await rumu.read({path: "a.png"});
                const b = await rumu.read({path: "b.png"});
                text("first:"); attach(b.attachments[1]); attach(b.attachments[1]);
                return a.text.split("\n").pop();"# }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content.text);
    assert!(
        out.content
            .text
            .ends_with("--- result ---\nfirst:\n[Image #1]\n[Image #1]\n[image: shot.png 800×600]"),
        "{}",
        out.content.text
    );
    let hashes: Vec<&str> = out
        .content
        .attachments
        .iter()
        .map(|a| a.hash.as_str())
        .collect();
    assert_eq!(
        hashes,
        ["c".repeat(64).as_str()],
        "unshown attachments are dropped"
    );

    // `attach(result)` shows every attachment of a call; a forged ref is refused.
    let host = FakeRuntime::arc();
    queue(
        &host,
        vec![ScriptedInner::ok(header).with_attachments(vec![image('a', "x"), image('b', "y")])],
    );
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"attach(await rumu.read({path: "a.png"}));
                attach({hash: "f".repeat(64)});"# }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(out.is_error);
    assert_eq!(
        out.content.attachments.len(),
        2,
        "output before the failure stays"
    );
    assert!(
        out.content
            .text
            .contains("[Image #1]\n[Image #2]\n--- error ---\nTypeError: attach()"),
        "{}",
        out.content.text
    );
}

/// Tools with an output schema resolve to their structured value, also when
/// the call is an error (a non-zero exit code is data, not a failure).
#[tokio::test]
async fn structured_results_resolve_to_values() {
    let mut bash = tool("bash");
    bash.output_schema = Some(json!({ "type": "object" }));
    let p = CodePlugin::new(&[bash]);
    let host = FakeRuntime::arc();
    let mut failed = ScriptedInner::err("exit code: 3\nboom").with_data(
        "bash.result",
        json!({ "exit_code": 3, "output": "boom", "truncated": false }),
    );
    failed.is_error = true;
    queue(&host, vec![failed]);
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"const r = await rumu.bash({path: "x"});
                return `${r.exit_code}:${r.output}:${r.truncated}`;"# }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(
        out.content.text.ends_with("--- result ---\n3:boom:false"),
        "{}",
        out.content.text
    );
    assert!(
        p.manifest().tools[0]
            .description
            .contains("Promise<Record<string, unknown>>")
    );
}

/// `text()` items come before the return value, in call order.
#[tokio::test]
async fn text_items_and_return_value() {
    let p = plugin(&[]);
    let out = run(&p, r#"text("a"); text({b: 1}); return "c";"#).await;
    assert!(
        out.content
            .text
            .ends_with("--- result ---\na\n{\n  \"b\": 1\n}\nc"),
        "{}",
        out.content.text
    );
}

/// Output beyond `max_output` keeps its start and end.
#[tokio::test]
async fn max_output_keeps_start_and_end() {
    let p = plugin(&[]);
    let host = FakeRuntime::arc();
    let out = p
        .call_tool(
            "code",
            json!({ "code": r#"return "HEAD" + "x".repeat(5000) + "TAIL";"#, "max_output": 1000 }),
            &host.cx(),
        )
        .await
        .unwrap();
    let text = &out.content.text;
    assert!(text.chars().count() <= 1000, "{}", text.len());
    assert!(text.starts_with("[code] ok"), "{text}");
    assert!(text.ends_with("TAIL"), "{text}");
    assert!(text.contains("[truncated"), "{text}");
}

/// Parallel tools overlap under `Promise.all`; others run one at a time.
#[tokio::test]
async fn parallel_tools_run_concurrently() {
    let mut read = tool("read");
    read.parallel = true;
    let p = CodePlugin::new(&[read, tool("edit")]);
    let code = |name: &str| {
        json!({ "code": format!(
            "return (await Promise.all([1, 2, 3].map(() => rumu.{name}({{path: 'a'}})))).join(',');"
        ) })
    };
    for (name, peak) in [("read", 3), ("edit", 1)] {
        let host = FakeRuntime::arc();
        queue(
            &host,
            (0..3)
                .map(|_| ScriptedInner::ok("x").with_delay(100))
                .collect(),
        );
        let out = p.call_tool("code", code(name), &host.cx()).await.unwrap();
        assert!(
            out.content.text.ends_with("--- result ---\nx,x,x"),
            "{}",
            out.content.text
        );
        let got = host.inner_peak.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(got, peak, "{name}");
    }
}

/// Calls still running when the script ends are cancelled, not orphaned.
#[tokio::test]
async fn unawaited_calls_are_cancelled() {
    let mut agent = tool("agent");
    agent.parallel = true;
    let p = CodePlugin::new(&[agent, tool("read")]);
    let host = FakeRuntime::arc();
    queue(
        &host,
        vec![
            ScriptedInner::ok("done").with_delay(30_000),
            ScriptedInner::ok("x"),
        ],
    );
    let started = Instant::now();
    // Awaiting a second call keeps the script alive until the first one has
    // reached the host: otherwise the script can end before the host ever
    // sees it, and there would be nothing to cancel.
    let out = p
        .call_tool(
            "code",
            json!({ "code": "rumu.agent({path: 'a'}); await rumu.read({path: 'b'}); return 'early';" }),
            &host.cx(),
        )
        .await
        .unwrap();
    assert!(
        out.content.text.ends_with("--- result ---\nearly"),
        "{}",
        out.content.text
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(*host.cancelled.lock().unwrap(), ["call-1"]);
    assert_eq!(
        host.inner_aborted.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
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
