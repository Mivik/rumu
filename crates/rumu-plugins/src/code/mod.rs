//! The `code` plugin: model-authored JavaScript that composes other tools.
//!
//! Hybrid mode: every direct tool stays available; `code` is one more tool.
//! A script runs in a fresh QuickJS runtime on its own OS thread (see
//! [`engine`]) and reaches the rest of rumu only through `Cx::invoke`, so
//! every internal call passes the same `tool.pre` / `tool.post` hooks as a
//! direct call and queues its appends to the enclosing batch's safe point.
//! Internal calls never become nodes.
//!
//! The tool description embeds a deterministic API stub ([`stubs`]): same
//! plugin set → byte-identical text (cache contract).

mod engine;
mod stubs;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rumu_core::{
    CODE_DENIED_TOOLS, Cx, Manifest, Plugin, ToolError, ToolOutput, ToolResult, ToolResultExt,
    ToolSpec, parse_args, tool_schema,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodeArgs {
    /// JavaScript async function body; use `await rumu.<tool>(args)`, `await rumu.call(name, args)`, and `return` the final value
    code: String,
    /// seconds, default 60, max 600
    timeout: Option<u64>,
}

/// Default / maximum `timeout` argument, in seconds (same as `bash`).
const CODE_TIMEOUT_DEFAULT: u64 = 60;
const CODE_TIMEOUT_MAX: u64 = 600;

const DESCRIPTION: &str = "Run JavaScript that composes other tools. Submit an async function \
body: `await rumu.<tool>(args)` calls a tool and returns its text output (`await rumu.call(name, \
args)` for any tool). Intermediate results stay inside the script — only the returned value and \
console output come back. Use it for loops, filtering/aggregating large outputs, or multi-step \
work where only the conclusion matters; for one-off work call the tool directly. Calls run \
sequentially, except `agent`, which may run concurrently (e.g. with `Promise.all`). Return a \
string (shown as-is), any other value (pretty-printed JSON), or \
`undefined` (no result section). A tool failure throws `ToolError` (blocked calls throw \
`BlockedError`, a subclass) with `message` = the output text / block reason and `tool` = the \
name; uncaught, the script fails. `console.log/info/warn/error` output is included.";

pub struct CodePlugin {
    manifest: Manifest,
    /// Tool names offered as `rumu.<name>` methods (sorted, denied filtered).
    api: Vec<String>,
}

impl CodePlugin {
    /// Build the plugin from every *other* plugin's tools. Call it last in
    /// `load_all` so stdio plugins are included too.
    pub fn new(tools: &[ToolSpec]) -> Self {
        let mut tools: Vec<ToolSpec> = tools
            .iter()
            .filter(|t| !CODE_DENIED_TOOLS.contains(&t.name.as_str()))
            .cloned()
            .collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools.dedup_by(|a, b| a.name == b.name);
        let api: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        let description = format!("{DESCRIPTION}\n\nAPI:\n{}", stubs::render(&tools));
        Self {
            manifest: Manifest {
                name: "code".into(),
                instructions: None,
                tools: vec![ToolSpec {
                    name: "code".into(),
                    description,
                    schema: tool_schema::<CodeArgs>(),
                    background: false,
                }],
                hooks: vec![],
            },
            api,
        }
    }
}

#[async_trait]
impl Plugin for CodePlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    async fn call_tool(&self, name: &str, args: Value, cx: &Cx) -> rumu_core::Result<ToolOutput> {
        self.code_tool(name, args, cx).await.finish()
    }
}

impl CodePlugin {
    async fn code_tool(&self, name: &str, args: Value, cx: &Cx) -> ToolResult<ToolOutput> {
        if name != "code" {
            return Err(ToolError::model(format!(
                "code plugin has no tool `{name}`"
            )));
        }
        let args: CodeArgs = parse_args("code", &args)?;
        let timeout = args
            .timeout
            .unwrap_or(CODE_TIMEOUT_DEFAULT)
            .clamp(1, CODE_TIMEOUT_MAX);
        if cx.call().is_none() {
            return Err(ToolError::model(
                "code: no call id in context (must be called as a tool)",
            ));
        }
        Ok(run_script(cx, &args.code, timeout, &self.api).await)
    }
}

/// Sets the engine's cancel flag if the future is dropped (abort / timeout):
/// the JS thread then unwinds out of `recv_timeout` or the interrupt handler.
struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn run_script(cx: &Cx, code: &str, timeout: u64, api: &[String]) -> ToolOutput {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(timeout);
    let cancel = Arc::new(AtomicBool::new(false));
    let _guard = CancelOnDrop(cancel.clone());

    let (req_tx, mut req_rx) = tokio::sync::mpsc::channel::<engine::EngineReq>(8);
    let (reply_tx, reply_rx) = std::sync::mpsc::channel::<engine::EngineReply>();
    let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
    let handle = engine::spawn(
        code.to_owned(),
        api.to_vec(),
        deadline,
        cancel.clone(),
        req_tx,
        reply_rx,
        done_tx,
    );

    let mut calls: u64 = 0;
    let blocked = Arc::new(AtomicU64::new(0));
    let mut reqs_open = true;
    let mut timed_out = false;
    let outcome = loop {
        tokio::select! {
            maybe = req_rx.recv(), if reqs_open => {
                match maybe {
                    Some(req) => {
                        calls += 1;
                        if req.name == "agent" {
                            // Agent calls are the one fan-out: let sibling
                            // agents run concurrently. Everything else stays
                            // strictly serial (deterministic order).
                            let cx = cx.clone();
                            let reply_tx = reply_tx.clone();
                            let blocked = blocked.clone();
                            tokio::spawn(async move {
                                let (json, was_blocked) =
                                    answer_cx(&cx, &req.name, &req.args_json).await;
                                if was_blocked {
                                    blocked.fetch_add(1, Ordering::Relaxed);
                                }
                                let _ = reply_tx.send(engine::EngineReply {
                                    ticket: req.ticket,
                                    json,
                                });
                            });
                        } else {
                            let ticket = req.ticket;
                            let (json, was_blocked) =
                                answer_cx(cx, &req.name, &req.args_json).await;
                            if was_blocked {
                                blocked.fetch_add(1, Ordering::Relaxed);
                            }
                            // The std channel never blocks; the engine polls it.
                            let _ = reply_tx.send(engine::EngineReply { ticket, json });
                        }
                    }
                    None => reqs_open = false,
                }
            }
            res = &mut done_rx => {
                break match res {
                    Ok(o) => o,
                    Err(_) => engine::EngineOutcome {
                        outcome: engine::ScriptOutcome::Error {
                            name: "Error".into(),
                            message: "the code engine thread panicked".into(),
                            stack: String::new(),
                        },
                        console: String::new(),
                    },
                };
            }
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                timed_out = true;
                cancel.store(true, Ordering::Relaxed);
                // Give the engine a moment to unwind and report its console.
                break match tokio::time::timeout(Duration::from_secs(1), &mut done_rx).await {
                    Ok(Ok(o)) => o,
                    _ => engine::EngineOutcome {
                        outcome: engine::ScriptOutcome::Interrupted,
                        console: String::new(),
                    },
                };
            }
        }
    };

    // On a normal exit the engine thread is done; on a timeout it may still be
    // unwinding, so it is detached (the cancel flag is set).
    if !timed_out {
        let _ = handle.join();
    }
    format_outcome(
        outcome,
        calls,
        blocked.load(Ordering::Relaxed),
        started.elapsed().as_millis() as u64,
        timeout,
        timed_out,
    )
}

/// Answer one submitted call. Blocked calls return the hook's reason as the
/// text so the prelude can throw a `BlockedError(message = reason)`.
async fn answer_cx(cx: &Cx, name: &str, args_json: &str) -> (String, bool) {
    let args: Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => {
            return (
                json!({ "ok": false, "text": format!("bad arguments: {e}") }).to_string(),
                false,
            );
        }
    };
    match cx.invoke(name, args).await {
        Ok(inner) => {
            let blocked = inner.blocked.is_some();
            let text = if blocked {
                inner
                    .blocked
                    .clone()
                    .unwrap_or_else(|| inner.output.content.text.clone())
            } else {
                inner.output.content.text.clone()
            };
            let reply = if blocked {
                json!({ "ok": false, "blocked": true, "text": text })
            } else if inner.output.is_error {
                json!({ "ok": false, "text": text })
            } else {
                json!({ "ok": true, "text": text })
            };
            (reply.to_string(), blocked)
        }
        Err(e) => (
            json!({ "ok": false, "text": format!("tool `{name}` failed: {e}") }).to_string(),
            false,
        ),
    }
}

fn format_outcome(
    outcome: engine::EngineOutcome,
    calls: u64,
    blocked: u64,
    dur_ms: u64,
    timeout: u64,
    timed_out: bool,
) -> ToolOutput {
    use engine::{ResultValue, ScriptOutcome};

    // A script that finished just as the deadline fired has a real result.
    let is_timeout = timed_out && !matches!(outcome.outcome, ScriptOutcome::Done(_));
    let stats = format!("{} · {dur_ms} ms", call_count(calls, blocked));
    let mut text = String::new();
    let mut is_error = false;
    if is_timeout {
        is_error = true;
        text.push_str(&format!("[code] timeout after {timeout} s"));
    } else {
        match &outcome.outcome {
            ScriptOutcome::Done(_) => text.push_str(&format!("[code] ok · {stats}")),
            ScriptOutcome::Error { .. } | ScriptOutcome::DidNotSettle => {
                is_error = true;
                text.push_str(&format!("[code] error · {stats}"));
            }
            ScriptOutcome::Interrupted => {
                is_error = true;
                text.push_str(&format!("[code] timeout after {timeout} s"));
            }
        }
    }
    if !outcome.console.is_empty() {
        text.push_str("\n--- console ---\n");
        text.push_str(&outcome.console);
    }
    if !is_timeout {
        match &outcome.outcome {
            ScriptOutcome::Done(ResultValue::Undefined) => {}
            ScriptOutcome::Done(ResultValue::String(s)) => {
                text.push_str("\n--- result ---\n");
                text.push_str(s);
            }
            ScriptOutcome::Done(ResultValue::Json(j)) => {
                text.push_str("\n--- result ---\n");
                text.push_str(j);
            }
            ScriptOutcome::Error {
                name,
                message,
                stack,
            } => {
                text.push_str("\n--- error ---\n");
                text.push_str(&format!("{name}: {message}"));
                if !stack.is_empty() {
                    text.push('\n');
                    text.push_str(stack);
                }
            }
            ScriptOutcome::DidNotSettle => {
                text.push_str("\n--- error ---\nError: script did not settle");
            }
            ScriptOutcome::Interrupted => {}
        }
    }
    ToolOutput {
        content: rumu_core::Content::text(text),
        is_error,
        ..Default::default()
    }
}

fn call_count(calls: u64, blocked: u64) -> String {
    let n = format!("{calls} call{}", if calls == 1 { "" } else { "s" });
    if blocked > 0 {
        format!("{n} ({blocked} blocked)")
    } else {
        n
    }
}
