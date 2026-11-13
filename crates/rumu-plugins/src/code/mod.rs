//! The `code` plugin: model-authored JavaScript that composes other tools.
//!
//! Hybrid mode: every direct tool stays available; `code` is one more tool.
//! A script runs in a fresh QuickJS runtime on its own OS thread (see
//! [`engine`]) and reaches the rest of rumu only through `Cx::invoke`, so
//! every internal call passes the same `tool.pre` / `tool.post` hooks as a
//! direct call and queues its appends to the enclosing batch's safe point.
//! Internal calls are recorded as `invoke` result nodes the model never
//! sees (the UI nests them under the `code` row). Only what the script
//! outputs (`text()`, `attach()`, `return`, console) reaches the model.
//!
//! The tool description embeds a deterministic API stub ([`stubs`]): same
//! plugin set → byte-identical text (cache contract).

mod engine;
mod stubs;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rumu_core::{
    Attachment, CODE_DENIED_TOOLS, Cx, Manifest, OUTPUT_LIMIT, Plugin, ToolError, ToolOutput,
    ToolResult, ToolResultExt, ToolSpec, parse_args, tool_schema, truncate_middle,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CodeArgs {
    /// JavaScript async function body; use `await rumu.<tool>(args)`, `await rumu.call(name, args)`, and `return` the final value
    code: String,
    /// seconds, default 60, max 600
    timeout: Option<u64>,
    /// characters of output to keep, default and max 20000; longer output keeps its start and end
    max_output: Option<usize>,
}

/// Default / maximum `timeout` argument, in seconds (same as `bash`).
const CODE_TIMEOUT_DEFAULT: u64 = 60;
const CODE_TIMEOUT_MAX: u64 = 600;
/// Smallest accepted `max_output` (room for the header and a cut marker).
const MAX_OUTPUT_MIN: usize = 500;
/// How long a finished script waits for its cancelled leftover calls.
const ORPHAN_GRACE: Duration = Duration::from_secs(5);

const DESCRIPTION: &str = "Run JavaScript that composes other tools. Submit an async function \
body: `await rumu.<tool>(args)` calls a tool (`await rumu.call(name, args)` for any tool). \
Intermediate results stay inside the script — only its output comes back: `text(value)` and the \
`return` value (strings as-is, other values as JSON), `attach(img)`, and \
`console.log/info/warn/error` lines. Use it for loops, filtering/aggregating large outputs, or \
multi-step work where only the conclusion matters.\n\n\
DO NOT USE THIS unless it clearly pays off: ordinary calls — reading a file, one search, one \
command, one edit, or a few independent calls — go directly to the tool, never wrapped in a \
script.\n\n\
A call resolves to the tool's text output (up to ~1M chars with the middle cut — far more than a \
direct call shows), to the value its signature names (e.g. `bash` → `{ exit_code, output, \
truncated }`, also for a non-zero exit), or to `{ text, attachments }` when the tool returned media. \
Attachments reach you only through `attach(img)` (`attach(result)` shows all of a result's \
attachments); they appear as `[Image #N]` anchors in the output.
Tools marked (parallel) run concurrently under `Promise.all`; other calls run one at a time, in \
order. Calls still running when the script ends are cancelled. A tool failure throws `ToolError` \
(blocked calls throw `BlockedError`, a subclass) with `message` = the output text / block reason \
and `tool` = the name; uncaught, the script fails and keeps its output so far. Output beyond \
`max_output` characters keeps its start and end.";

pub struct CodePlugin {
    manifest: Manifest,
    /// Tool names offered as `rumu.<name>` methods (sorted, denied filtered).
    api: Vec<String>,
    /// Tools that may run concurrently ([`ToolSpec::parallel`]).
    parallel: HashSet<String>,
    /// Tools whose structured result a script receives
    /// ([`ToolSpec::output_schema`]).
    structured: HashSet<String>,
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
        let names = |keep: fn(&ToolSpec) -> bool| {
            tools
                .iter()
                .filter(move |t| keep(t))
                .map(|t| t.name.clone())
        };
        let api = names(|_| true).collect();
        let parallel = names(|t| t.parallel).collect();
        let structured = names(|t| t.output_schema.is_some()).collect();
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
                    output_schema: None,
                    parallel: false,
                }],
                hooks: vec![],
            },
            api,
            parallel,
            structured,
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
        let max_output = args
            .max_output
            .unwrap_or(OUTPUT_LIMIT)
            .clamp(MAX_OUTPUT_MIN, OUTPUT_LIMIT);
        if cx.call().is_none() {
            return Err(ToolError::model(
                "code: no call id in context (must be called as a tool)",
            ));
        }
        let mut out = self.run_script(cx, &args.code, timeout).await;
        out.content.text = truncate_middle(std::mem::take(&mut out.content.text), max_output);
        Ok(out)
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

/// The script's in-flight internal calls. Dropped early (the turn was
/// aborted), it cancels them and lets them unwind detached — aborting a
/// sub-agent's future mid-flight would leave its branch Active.
struct Inflight {
    set: JoinSet<()>,
    cx: Cx,
}

impl Drop for Inflight {
    fn drop(&mut self) {
        if !self.set.is_empty() {
            self.cx.cancel_invokes();
            self.set.detach_all();
        }
    }
}

/// What the internal calls produced besides their replies.
#[derive(Default)]
struct Seen {
    blocked: u64,
    /// Every attachment a call returned, by hash: `attach()` may show these.
    attachments: HashMap<String, Attachment>,
}

impl CodePlugin {
    async fn run_script(&self, cx: &Cx, code: &str, timeout: u64) -> ToolOutput {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(timeout);
        let cancel = Arc::new(AtomicBool::new(false));
        let _guard = CancelOnDrop(cancel.clone());

        let (req_tx, mut req_rx) = tokio::sync::mpsc::channel::<engine::EngineReq>(8);
        let (reply_tx, reply_rx) = std::sync::mpsc::channel::<engine::EngineReply>();
        let (done_tx, mut done_rx) = tokio::sync::oneshot::channel();
        let handle = engine::spawn(
            code.to_owned(),
            self.api.clone(),
            deadline,
            cancel.clone(),
            req_tx,
            reply_rx,
            done_tx,
        );

        let seen = Arc::new(std::sync::Mutex::new(Seen::default()));
        let parallel_answer = |req: engine::EngineReq| {
            let structured = self.structured.contains(&req.name);
            answer(cx.clone(), req, structured, seen.clone(), reply_tx.clone())
        };
        let mut inflight = Inflight {
            set: JoinSet::new(),
            cx: cx.clone(),
        };
        // Non-parallel calls go through one worker, strictly in order.
        let (serial_tx, mut serial_rx) =
            tokio::sync::mpsc::unbounded_channel::<engine::EngineReq>();
        inflight.set.spawn({
            let cx = cx.clone();
            let seen = seen.clone();
            let reply_tx = reply_tx.clone();
            let structured = self.structured.clone();
            async move {
                while let Some(req) = serial_rx.recv().await {
                    let s = structured.contains(&req.name);
                    answer(cx.clone(), req, s, seen.clone(), reply_tx.clone()).await;
                }
            }
        });

        let mut calls: u64 = 0;
        let mut reqs_open = true;
        let mut timed_out = false;
        let outcome = loop {
            tokio::select! {
                maybe = req_rx.recv(), if reqs_open => match maybe {
                    Some(req) => {
                        calls += 1;
                        if self.parallel.contains(&req.name) {
                            inflight.set.spawn(parallel_answer(req));
                        } else {
                            let _ = serial_tx.send(req);
                        }
                    }
                    None => reqs_open = false,
                },
                res = &mut done_rx => {
                    break res.unwrap_or_else(|_| engine::EngineOutcome {
                        outcome: engine::ScriptOutcome::Error {
                            name: "Error".into(),
                            message: "the code engine thread panicked".into(),
                            stack: String::new(),
                        },
                        console: String::new(),
                        items: Vec::new(),
                    });
                }
                _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                    timed_out = true;
                    cancel.store(true, Ordering::Relaxed);
                    // Give the engine a moment to unwind and report its output.
                    break match tokio::time::timeout(Duration::from_secs(1), &mut done_rx).await {
                        Ok(Ok(o)) => o,
                        _ => engine::EngineOutcome {
                            outcome: engine::ScriptOutcome::Interrupted,
                            console: String::new(),
                            items: Vec::new(),
                        },
                    };
                }
            }
        };

        // Whatever still runs was never awaited (or the script timed out):
        // cancel it and wait briefly so sub-agents finish as aborted.
        drop(serial_tx);
        cx.cancel_invokes();
        let _ = tokio::time::timeout(ORPHAN_GRACE, async {
            while inflight.set.join_next().await.is_some() {}
        })
        .await;
        // On a normal exit the engine thread is done; on a timeout it may
        // still be unwinding, so it is detached (the cancel flag is set).
        if !timed_out {
            let _ = handle.join();
        }
        let seen = std::mem::take(&mut *seen.lock().unwrap());
        format_outcome(
            outcome,
            calls,
            &seen,
            started.elapsed().as_millis() as u64,
            timeout,
            timed_out,
        )
    }
}

/// Answer one submitted call on the reply channel. Blocked calls return the
/// hook's reason as the text so the prelude can throw a
/// `BlockedError(message = reason)`. A tool with a structured result replies
/// with its value (even on `is_error`, e.g. a non-zero exit); otherwise the
/// text is [`rumu_core::Content::rendered`], plus attachment refs when the
/// call returned media.
async fn answer(
    cx: Cx,
    req: engine::EngineReq,
    structured: bool,
    seen: Arc<std::sync::Mutex<Seen>>,
    reply: std::sync::mpsc::Sender<engine::EngineReply>,
) {
    let json = match serde_json::from_str::<Value>(&req.args_json) {
        Err(e) => json!({ "ok": false, "text": format!("bad arguments: {e}") }),
        Ok(args) => match cx.invoke(&req.name, args).await {
            Err(e) => json!({ "ok": false, "text": format!("tool `{}` failed: {e}", req.name) }),
            Ok(inner) => {
                let content = &inner.output.content;
                let mut seen = seen.lock().unwrap();
                if let Some(reason) = inner.blocked {
                    seen.blocked += 1;
                    json!({ "ok": false, "blocked": true, "text": reason })
                } else if let Some(data) = content.data.as_ref().filter(|_| structured) {
                    json!({ "ok": true, "value": data.data })
                } else if inner.output.is_error {
                    json!({ "ok": false, "text": content.rendered() })
                } else if content.attachments.is_empty() {
                    json!({ "ok": true, "text": content.rendered() })
                } else {
                    let attachments: Vec<Value> = content
                        .attachments
                        .iter()
                        .map(|a| {
                            seen.attachments.insert(a.hash.clone(), a.clone());
                            serde_json::to_value(a).unwrap_or_default()
                        })
                        .collect();
                    json!({ "ok": true, "text": content.rendered(), "attachments": attachments })
                }
            }
        },
    };
    // The std channel never blocks; the engine polls it.
    let _ = reply.send(engine::EngineReply {
        ticket: req.ticket,
        json: json.to_string(),
    });
}

fn format_outcome(
    outcome: engine::EngineOutcome,
    calls: u64,
    seen: &Seen,
    dur_ms: u64,
    timeout: u64,
    timed_out: bool,
) -> ToolOutput {
    use engine::{Item, ResultValue, ScriptOutcome};

    // A script that finished just as the deadline fired has a real result.
    let is_timeout = timed_out && !matches!(outcome.outcome, ScriptOutcome::Done(_));
    let stats = format!("{} · {dur_ms} ms", call_count(calls, seen.blocked));
    let mut text = String::new();
    let mut is_error = true;
    match &outcome.outcome {
        _ if is_timeout => text.push_str(&format!("[code] timeout after {timeout} s")),
        ScriptOutcome::Interrupted => text.push_str(&format!("[code] timeout after {timeout} s")),
        ScriptOutcome::Done(_) => {
            is_error = false;
            text.push_str(&format!("[code] ok · {stats}"));
        }
        ScriptOutcome::Error { .. } | ScriptOutcome::DidNotSettle => {
            text.push_str(&format!("[code] error · {stats}"));
        }
    }
    if !outcome.console.is_empty() {
        text.push_str("\n--- console ---\n");
        text.push_str(&outcome.console);
    }

    // `text()` / `attach()` items in order, then the return value. An
    // attachment becomes an `[Image #N]` anchor; showing one twice attaches
    // it once.
    let mut parts: Vec<String> = Vec::new();
    let mut attachments: Vec<Attachment> = Vec::new();
    for item in &outcome.items {
        match item {
            Item::Text(s) => parts.push(s.clone()),
            Item::Attachment(hash) => {
                let Some(a) = seen.attachments.get(hash) else {
                    continue;
                };
                let n = match attachments.iter().position(|x| &x.hash == hash) {
                    Some(i) => i + 1,
                    None => {
                        attachments.push(a.clone());
                        attachments.len()
                    }
                };
                parts.push(format!("[Image #{n}]"));
            }
        }
    }
    if let ScriptOutcome::Done(ResultValue::String(s) | ResultValue::Json(s)) = &outcome.outcome
        && !is_timeout
    {
        parts.push(s.clone());
    }
    if !parts.is_empty() {
        text.push_str("\n--- result ---\n");
        text.push_str(&parts.join("\n"));
    }
    if !is_timeout {
        match &outcome.outcome {
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
            _ => {}
        }
    }
    ToolOutput {
        content: rumu_core::Content::text(text).with_attachments(attachments),
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
