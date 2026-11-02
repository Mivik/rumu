//! The QuickJS engine thread.
//!
//! `Runtime` / `Context` are not `Send`, while `Plugin::call_tool`'s future
//! must be. Each `code` call therefore gets a dedicated OS thread with a fresh
//! runtime; the async side talks to it over channels and never touches a JS
//! value.
//!
//! The native bridge is ticket-based: `__call` submits a request and returns a
//! ticket immediately, and the prelude resolves the matching Promise when a
//! reply arrives. The engine loop drives the QuickJS job queue and settles
//! promises between jobs, which also makes script-side `Promise.all` possible
//! without the `futures` feature.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use rquickjs::promise::PromiseState;
use rquickjs::{Context, Ctx, Function, Object, Promise, Runtime};
use serde_json::{Value, json};

/// Per-script cap on submitted tool calls.
pub(super) const CALL_LIMIT: u32 = 200;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_SIZE: usize = 1024 * 1024;
const REPLY_POLL: Duration = Duration::from_millis(25);

/// One submitted internal call. The async side answers on the shared reply
/// channel, keyed by `ticket`.
pub(super) struct EngineReq {
    pub ticket: u32,
    pub name: String,
    pub args_json: String,
}

pub(super) struct EngineReply {
    pub ticket: u32,
    /// `{"ok":true,"text":…}` / `{"ok":false,"blocked":bool,"text":…}`
    pub json: String,
}

pub(super) enum ResultValue {
    Undefined,
    String(String),
    Json(String),
}

pub(super) enum ScriptOutcome {
    Done(ResultValue),
    Error {
        name: String,
        message: String,
        stack: String,
    },
    /// The script's promise never settled and no more jobs are runnable.
    DidNotSettle,
    /// Cancelled or past the deadline (the caller formats the timeout line).
    Interrupted,
}

/// One `text()` / `attach()` output item, in call order.
pub(super) enum Item {
    Text(String),
    /// The hash of an attachment a call returned.
    Attachment(String),
}

pub(super) struct EngineOutcome {
    pub outcome: ScriptOutcome,
    pub console: String,
    pub items: Vec<Item>,
}

/// Spawn the engine thread. The thread owns its channel ends and the runtime;
/// `done` receives exactly one outcome unless the thread panics.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn(
    code: String,
    api: Vec<String>,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    req_tx: tokio::sync::mpsc::Sender<EngineReq>,
    reply_rx: mpsc::Receiver<EngineReply>,
    done: tokio::sync::oneshot::Sender<EngineOutcome>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("rumu-code".into())
        .spawn(move || {
            let outcome = run(&code, &api, deadline, &cancel, req_tx, reply_rx);
            let _ = done.send(outcome);
        })
        .expect("spawning the code engine thread")
}

fn run(
    code: &str,
    api: &[String],
    deadline: Instant,
    cancel: &Arc<AtomicBool>,
    req_tx: tokio::sync::mpsc::Sender<EngineReq>,
    reply_rx: mpsc::Receiver<EngineReply>,
) -> EngineOutcome {
    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return plain_error(format!("could not create the JS runtime: {e}")),
    };
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(STACK_SIZE);
    {
        let cancel = cancel.clone();
        rt.set_interrupt_handler(Some(Box::new(move || {
            cancel.load(Ordering::Relaxed) || Instant::now() >= deadline
        })));
    }
    let ctx = match Context::full(&rt) {
        Ok(ctx) => ctx,
        Err(e) => return plain_error(format!("could not create the JS context: {e}")),
    };
    ctx.with(|ctx| run_ctx(&ctx, code, api, deadline, cancel, req_tx, reply_rx))
}

fn run_ctx(
    ctx: &Ctx<'_>,
    code: &str,
    api: &[String],
    deadline: Instant,
    cancel: &Arc<AtomicBool>,
    req_tx: tokio::sync::mpsc::Sender<EngineReq>,
    reply_rx: mpsc::Receiver<EngineReply>,
) -> EngineOutcome {
    let ticket = Arc::new(AtomicU32::new(0));
    let accepted = ticket.clone();
    let submissions = Arc::new(AtomicU32::new(0));
    let native = {
        let ticket = ticket.clone();
        let submissions = submissions.clone();
        let cancel = cancel.clone();
        Function::new(
            ctx.clone(),
            move |name: String, args_json: String| -> String {
                if cancel.load(Ordering::Relaxed) || Instant::now() >= deadline {
                    return json!({ "ok": false, "text": "aborted" }).to_string();
                }
                if submissions.fetch_add(1, Ordering::Relaxed) >= CALL_LIMIT {
                    return json!({
                        "ok": false,
                        "text": format!("too many tool calls (limit {CALL_LIMIT})"),
                    })
                    .to_string();
                }
                let t = ticket.fetch_add(1, Ordering::Relaxed) + 1;
                let req = EngineReq {
                    ticket: t,
                    name,
                    args_json,
                };
                if req_tx.blocking_send(req).is_err() {
                    return json!({ "ok": false, "text": "code engine is gone" }).to_string();
                }
                json!({ "ok": true, "ticket": t }).to_string()
            },
        )
    };
    let native = match native {
        Ok(f) => f,
        Err(e) => return plain_error(format!("could not install the bridge: {e}")),
    };
    let api_json = serde_json::to_string(api).unwrap_or_else(|_| "[]".into());
    if let Err(e) = ctx
        .globals()
        .set("__call", native)
        .and_then(|()| ctx.globals().set("__api", api_json))
    {
        return plain_error(format!("could not install the bridge: {e}"));
    }

    let prelude: Object = match ctx.eval(PRELUDE) {
        Ok(o) => o,
        Err(e) => return plain_error(format!("could not initialize the script API: {e}")),
    };
    let settle: Function = match prelude.get("settle") {
        Ok(f) => f,
        Err(e) => return plain_error(format!("could not initialize the script API: {e}")),
    };
    let (logs, items): (Function, Function) = match prelude
        .get("logs")
        .and_then(|l| Ok((l, prelude.get("items")?)))
    {
        Ok(f) => f,
        Err(e) => return plain_error(format!("could not initialize the script API: {e}")),
    };
    let finish = |outcome| EngineOutcome {
        outcome,
        console: logs.call::<_, String>(()).unwrap_or_default(),
        items: parse_items(&items.call::<_, String>(()).unwrap_or_default()),
    };

    let promise: Promise = match ctx.eval(source(code)) {
        Ok(p) => p,
        Err(e) => {
            // The synchronous prologue of the script threw (syntax error,
            // `while (true) {}`, OOM): a deadline/cancel is a timeout, not a
            // script error.
            let outcome = if interrupted(cancel, deadline) {
                ScriptOutcome::Interrupted
            } else {
                let (name, message, stack) = describe(ctx, &e);
                ScriptOutcome::Error {
                    name,
                    message,
                    stack,
                }
            };
            return finish(outcome);
        }
    };

    let mut settled: u32 = 0;
    let outcome = loop {
        // Check cancel/deadline between jobs too: a self-rescheduling
        // microtask loop must not be able to spin past the timeout.
        while !interrupted(cancel, deadline) && ctx.execute_pending_job() {}
        match promise.state() {
            PromiseState::Resolved => {
                let out = match promise.result::<String>() {
                    Some(Ok(s)) => parse_result(&s),
                    Some(Err(e)) => {
                        let (name, message, stack) = describe(ctx, &e);
                        ScriptOutcome::Error {
                            name,
                            message,
                            stack,
                        }
                    }
                    None => ScriptOutcome::DidNotSettle,
                };
                break out;
            }
            PromiseState::Rejected => {
                // `result()` re-throws the rejection value; then catch it.
                let _ = promise.result::<String>();
                let (name, message, stack) = describe_pending(ctx);
                break ScriptOutcome::Error {
                    name,
                    message,
                    stack,
                };
            }
            PromiseState::Pending => {}
        }
        // Every accepted call has been answered and no job is runnable: the
        // script is waiting on a promise nothing can settle.
        if settled >= accepted.load(Ordering::Relaxed) {
            break ScriptOutcome::DidNotSettle;
        }
        if interrupted(cancel, deadline) {
            break ScriptOutcome::Interrupted;
        }
        match reply_rx.recv_timeout(REPLY_POLL) {
            Ok(reply) => {
                settled += 1;
                // A failed settle leaves the script waiting; the loop will
                // notice a cancel/deadline or exhaust the job queue.
                let _ = settle.call::<_, ()>((reply.ticket, reply.json));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // The async side is gone; the drop guard sets `cancel`, so
                // one more loop around observes it rather than spinning hot.
                std::thread::sleep(REPLY_POLL);
            }
        }
    };

    finish(outcome)
}

fn interrupted(cancel: &AtomicBool, deadline: Instant) -> bool {
    cancel.load(Ordering::Relaxed) || Instant::now() >= deadline
}

fn plain_error(message: String) -> EngineOutcome {
    EngineOutcome {
        outcome: ScriptOutcome::Error {
            name: "Error".into(),
            message,
            stack: String::new(),
        },
        console: String::new(),
        items: Vec::new(),
    }
}

fn parse_items(json: &str) -> Vec<Item> {
    let items: Vec<Value> = serde_json::from_str(json).unwrap_or_default();
    items
        .iter()
        .filter_map(|v| match (v.get("text"), v.get("attachment")) {
            (Some(Value::String(s)), _) => Some(Item::Text(s.clone())),
            (_, Some(Value::String(h))) => Some(Item::Attachment(h.clone())),
            _ => None,
        })
        .collect()
}

fn parse_result(json: &str) -> ScriptOutcome {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let field = |name: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match v.get("kind").and_then(Value::as_str) {
        Some("undefined") => ScriptOutcome::Done(ResultValue::Undefined),
        Some("string") => ScriptOutcome::Done(ResultValue::String(field("value"))),
        Some("json") => ScriptOutcome::Done(ResultValue::Json(field("value"))),
        Some("error") => ScriptOutcome::Error {
            name: field("name"),
            message: field("message"),
            stack: field("stack"),
        },
        // Should not happen (the wrapper always encodes), but never lose the
        // value silently.
        _ => ScriptOutcome::Done(ResultValue::Json(json.to_owned())),
    }
}

fn describe(ctx: &Ctx<'_>, error: &rquickjs::Error) -> (String, String, String) {
    if matches!(error, rquickjs::Error::Exception) {
        describe_pending(ctx)
    } else {
        ("Error".into(), error.to_string(), String::new())
    }
}

fn describe_pending(ctx: &Ctx<'_>) -> (String, String, String) {
    let v = ctx.catch();
    let Some(obj) = v.as_object() else {
        return ("Error".into(), format!("{v:?}"), String::new());
    };
    let name: String = obj.get("name").unwrap_or_else(|_| "Error".into());
    let message: String = obj.get("message").unwrap_or_default();
    let stack: String = obj.get("stack").unwrap_or_default();
    (name, message, stack)
}

/// Wrap the user's code as an async function body and encode the outcome as a
/// JSON string (the promise always resolves unless the engine is interrupted).
fn source(code: &str) -> String {
    format!(
        r#"(async () => {{
  const __enc = globalThis.__encode;
  const __encErr = globalThis.__encodeError;
  delete globalThis.__encode;
  delete globalThis.__encodeError;
  try {{
    const __v = await (async () => {{
{code}
    }})();
    return __enc(__v);
  }} catch (__e) {{
    return __encErr(__e);
  }}
}})()"#
    )
}

/// The script API. Kept as one string so the shape is easy to audit: only
/// `__call` reaches Rust, and it is deleted before user code runs.
const PRELUDE: &str = r#"(() => {
  "use strict";
  const native = globalThis.__call;
  delete globalThis.__call;
  const names = JSON.parse(globalThis.__api);
  delete globalThis.__api;

  const pending = new Map();
  const logs = [];
  let logChars = 0;
  const MAX_LOG_CHARS = 200000;

  class ToolError extends Error {
    constructor(message, tool) {
      super(String(message));
      this.name = "ToolError";
      this.tool = tool;
    }
  }
  class BlockedError extends ToolError {
    constructor(message, tool) {
      super(String(message), tool);
      this.name = "BlockedError";
    }
  }
  // Scripts catch these by name.
  globalThis.ToolError = ToolError;
  globalThis.BlockedError = BlockedError;

  const fmtJson = (v) => {
    try {
      const s = JSON.stringify(v, null, 2);
      return s === undefined ? String(v) : s;
    } catch (_) {
      return String(v);
    }
  };
  const fmt = (v) => {
    if (typeof v === "string") return v;
    try {
      const s = JSON.stringify(v);
      return s === undefined ? String(v) : s;
    } catch (_) {
      return String(v);
    }
  };
  const log = (args) => {
    const line = args.map(fmt).join(" ");
    if (logChars >= MAX_LOG_CHARS) return;
    const room = MAX_LOG_CHARS - logChars;
    const add = line.length <= room ? line : line.slice(0, room);
    logs.push(add);
    logChars += add.length + 1;
    if (add.length < line.length) logs.push("… [console truncated]");
  };
  // `text()` / `attach()` output items; `attachments` holds every attachment
  // a call returned (by hash), the only ones `attach()` accepts.
  const items = [];
  let itemChars = 0;
  const MAX_ITEM_CHARS = 4 * 1024 * 1024;
  const attachments = new Map();
  globalThis.text = (v) => {
    const s = typeof v === "string" ? v : fmtJson(v);
    itemChars += s.length;
    if (itemChars > MAX_ITEM_CHARS) {
      throw new Error("script output limit exceeded; filter or aggregate before text()");
    }
    items.push({ text: s });
  };
  globalThis.attach = (v) => {
    const list = Array.isArray(v) ? v : v && Array.isArray(v.attachments) ? v.attachments : [v];
    for (const a of list) {
      if (!a || typeof a !== "object" || !attachments.has(a.hash)) {
        throw new TypeError("attach(): pass an attachment returned by a tool call in this script");
      }
      items.push({ attachment: a.hash });
    }
  };

  globalThis.console = {
    log: (...a) => log(a),
    info: (...a) => log(a),
    warn: (...a) => log(a),
    error: (...a) => log(a),
  };

  const rumu = {};
  rumu.call = (name, args) => {
    if (typeof name !== "string") {
      return Promise.reject(new ToolError("tool name must be a string", String(name)));
    }
    let body;
    try {
      body = JSON.stringify(args === undefined ? {} : args);
    } catch (e) {
      return Promise.reject(e);
    }
    let r;
    try {
      r = JSON.parse(native(name, body));
    } catch (e) {
      return Promise.reject(e);
    }
    if (!r.ok) return Promise.reject(new ToolError(r.text || "tool call failed", name));
    return new Promise((resolve, reject) => {
      pending.set(r.ticket, { resolve, reject, tool: name });
    });
  };
  for (const name of names) {
    if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name)) {
      rumu[name] = (a) => rumu.call(name, a);
    }
  }
  Object.freeze(rumu);
  globalThis.rumu = rumu;

  globalThis.__encode = (v) => {
    if (v === undefined) return JSON.stringify({ kind: "undefined" });
    if (typeof v === "string") return JSON.stringify({ kind: "string", value: v });
    let s;
    try {
      s = JSON.stringify(v, null, 2);
    } catch (_) {
      s = String(v);
    }
    if (s === undefined) return JSON.stringify({ kind: "undefined" });
    return JSON.stringify({ kind: "json", value: s });
  };
  globalThis.__encodeError = (e) => {
    let name = "Error";
    let message = String(e);
    let stack = "";
    try {
      if (e && typeof e === "object") {
        name = String(e.name || "Error");
        message = String(e.message !== undefined ? e.message : e);
        stack = String(e.stack || "");
      }
    } catch (_) {}
    return JSON.stringify({ kind: "error", name, message, stack });
  };

  const settle = (ticket, json) => {
    const p = pending.get(ticket);
    if (!p) return;
    pending.delete(ticket);
    let r;
    try {
      r = JSON.parse(json);
    } catch (e) {
      p.reject(e);
      return;
    }
    if (r.blocked) p.reject(new BlockedError(r.text, p.tool));
    else if (!r.ok) p.reject(new ToolError(r.text, p.tool));
    else if (r.value !== undefined) p.resolve(r.value);
    else if (r.attachments) {
      for (const a of r.attachments) attachments.set(a.hash, true);
      p.resolve({ text: r.text, attachments: r.attachments });
    } else p.resolve(r.text);
  };

  return {
    settle,
    logs: () => logs.join("\n"),
    items: () => JSON.stringify(items),
  };
})()"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny harness: spawn the engine with a trivial bridge that answers
    /// every request with `answer`.
    fn run_engine(
        code: &str,
        timeout: Duration,
        answer: impl Fn(&EngineReq) -> String + Send + Sync + 'static,
    ) -> EngineOutcome {
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let (req_tx, mut req_rx) = tokio::sync::mpsc::channel(8);
        let (reply_tx, reply_rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + timeout;
        let api = vec!["read".to_owned()];
        let handle = spawn(
            code.to_owned(),
            api,
            deadline,
            cancel.clone(),
            req_tx,
            reply_rx,
            done_tx,
        );
        // Pump requests in a helper thread (the engine thread blocks on
        // replies; the test thread is the async side).
        let answer = Arc::new(answer);
        let pump = std::thread::spawn(move || {
            while let Some(req) = req_rx.blocking_recv() {
                let _ = reply_tx.send(EngineReply {
                    ticket: req.ticket,
                    json: answer(&req),
                });
            }
        });
        let outcome = futures_lite_block_on(done_rx);
        let _ = handle.join();
        pump.join().unwrap();
        outcome
    }

    /// Minimal single-future poll loop (avoids pulling an async runtime into
    /// a sync test thread).
    fn futures_lite_block_on<T>(rx: tokio::sync::oneshot::Receiver<T>) -> T {
        let mut rx = rx;
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            match std::future::Future::poll(std::pin::Pin::new(&mut rx), &mut cx) {
                std::task::Poll::Ready(Ok(v)) => return v,
                std::task::Poll::Ready(Err(e)) => panic!("engine thread vanished: {e}"),
                std::task::Poll::Pending => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn returns_a_value_and_console() {
        let out = run_engine(
            "console.log('hi', 1); return {a: 1};",
            Duration::from_secs(5),
            |_| json!({"ok": true, "text": "x"}).to_string(),
        );
        match out.outcome {
            ScriptOutcome::Done(ResultValue::Json(s)) => assert_eq!(s, "{\n  \"a\": 1\n}"),
            _ => panic!("unexpected outcome"),
        }
        assert_eq!(out.console, "hi 1");
    }

    #[test]
    fn tool_call_round_trip_and_errors() {
        let out = run_engine(
            r#"
            const a = await rumu.read({});
            try { await rumu.call("bad", {}); } catch (e) { return (e instanceof Error) + ":" + e.name + ":" + e.message + ":" + e.tool; }
            return a;
            "#,
            Duration::from_secs(5),
            |req| {
                if req.name == "bad" {
                    json!({"ok": false, "text": "boom"}).to_string()
                } else {
                    json!({"ok": true, "text": "file"}).to_string()
                }
            },
        );
        match out.outcome {
            ScriptOutcome::Done(ResultValue::String(s)) => {
                assert_eq!(s, "true:ToolError:boom:bad")
            }
            _ => panic!("unexpected outcome"),
        }
    }

    #[test]
    fn deadline_interrupts_an_infinite_loop() {
        let out = run_engine("while (true) {}", Duration::from_millis(200), |_| {
            json!({"ok": true, "text": ""}).to_string()
        });
        assert!(matches!(out.outcome, ScriptOutcome::Interrupted));
    }

    #[test]
    fn unbound_promise_does_not_settle() {
        let out = run_engine(
            "return new Promise(() => {});",
            Duration::from_secs(2),
            |_| json!({"ok": true, "text": ""}).to_string(),
        );
        assert!(matches!(out.outcome, ScriptOutcome::DidNotSettle));
    }

    #[test]
    fn native_bridge_is_hidden() {
        let out = run_engine(
            "return typeof __call + ':' + typeof globalThis.__api + ':' + typeof rumu.read + ':' + typeof rumu.call;",
            Duration::from_secs(5),
            |_| json!({"ok": true, "text": ""}).to_string(),
        );
        match out.outcome {
            ScriptOutcome::Done(ResultValue::String(s)) => {
                assert_eq!(s, "undefined:undefined:function:function")
            }
            _ => panic!("unexpected outcome"),
        }
    }

    /// The future's drop guard sets `cancel`; the JS thread must leave the
    /// interruptible loop within a bounded time (no leaked thread).
    #[test]
    fn cancel_stops_the_engine_thread() {
        let (req_tx, _req_rx) = tokio::sync::mpsc::channel(8);
        let (_reply_tx, reply_rx) = mpsc::channel();
        let (done_tx, _done_rx) = tokio::sync::oneshot::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            "while (true) {}".to_owned(),
            vec![],
            Instant::now() + Duration::from_secs(60),
            cancel.clone(),
            req_tx,
            reply_rx,
            done_tx,
        );
        std::thread::sleep(Duration::from_millis(100));
        assert!(!handle.is_finished(), "still spinning before cancel");
        cancel.store(true, Ordering::Relaxed);
        let start = Instant::now();
        while !handle.is_finished() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished(), "the engine exited after cancel");
        handle.join().unwrap();
    }
}
