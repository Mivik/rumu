//! Hook dispatch.

use std::sync::Arc;

use rumu_core::{BranchId, Cx, Decision, Event, HookOutput, kind, now};
use serde_json::{Value, json};

use crate::{Kernel, RegisteredHook, Result, fields, host::KernelRuntime};

impl Kernel {
    /// A hook-facing context for `ev`, bound to the owning plugin's name.
    fn cx_for(self: &Arc<Self>, ev: &Event, plugin: &str) -> Cx {
        Cx::new(
            ev.session.clone(),
            ev.branch.clone(),
            None,
            Some(plugin.to_owned()),
            0,
            Arc::new(KernelRuntime(self.clone())),
        )
    }

    /// Call one hook with the fail-closed timeout policy.
    async fn call_hook(
        self: &Arc<Self>,
        h: &RegisteredHook,
        ev: &Event,
        interceptable: bool,
        hook_timeout: std::time::Duration,
    ) -> HookOutput {
        let plugin = self.plugins[h.plugin].clone();
        let cx = self.cx_for(ev, &h.plugin_name);
        let who = || format!("{}/{}", h.plugin_name, h.spec.id);
        match tokio::time::timeout(hook_timeout, plugin.on_hook(&h.spec.id, ev, &cx)).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => fail_closed(interceptable, &who(), format!("failed: {e}")),
            Err(_) => fail_closed(
                interceptable,
                &who(),
                format!("timed out after {hook_timeout:?}"),
            ),
        }
    }

    /// Dispatch an event to all matching hooks. Returns the block reason when
    /// the event was blocked (only possible for interceptable events).
    ///
    /// Intercept dispatch is two-phase: rewrites are applied first (re-matching
    /// after each patch), then hooks whose verdict predates the last rewrite
    /// are re-called with the final payload — a guard always judges what will
    /// actually run, and a rewrite can neither bypass a guard nor be vetoed by
    /// a stale block.
    pub(crate) async fn dispatch(
        self: &Arc<Self>,
        ev: &mut Event,
        interceptable: bool,
    ) -> Result<Option<String>> {
        let hooks = &self.hooks;
        let state = self.state_of_nodes(&self.pending_path_nodes(&ev.branch)?);
        let hook_timeout = self.inner.lock().unwrap().hook_timeout;

        // `hooks` is pre-sorted by (priority, registration order).
        // `verdict[i]` = output returned by hook i (None = not called yet);
        // `called_at[i]` / `last_rewrite_at` order the calls so phase 2 can
        // find stale verdicts.
        let mut verdict: Vec<Option<HookOutput>> = vec![None; hooks.len()];
        let mut called_at: Vec<Option<u32>> = vec![None; hooks.len()];
        let mut last_rewrite_at: u32 = 0;
        let mut calls: u32 = 0;

        // Phase 1: call hooks in priority order, applying rewrites as they
        // arrive. The match set is re-evaluated after every patch, so hooks
        // that only match the rewritten payload still run; every hook is
        // called at most once per dispatch.
        while let Some(idx) = hooks
            .iter()
            .enumerate()
            .find(|(i, h)| verdict[*i].is_none() && h.pred.matches(&ev.fields))
            .map(|(i, _)| i)
        {
            let h = &hooks[idx];
            calls += 1;

            // Fail-closed: a malfunctioning hook on an interceptable event
            // becomes a synthetic block.
            let output = self.call_hook(h, ev, interceptable, hook_timeout).await;
            self.push_hook_fired(ev, &h.plugin_name, &h.spec.id, &output);
            called_at[idx] = Some(calls);

            if !output.effects.is_empty()
                && let Err(e) = self
                    .deliver_effects(&ev.branch, &h.plugin_name, output.effects.clone())
                    .await
            {
                self.warn(format!(
                    "delivering hook effects from `{}/{}` failed: {e}",
                    h.plugin_name, h.spec.id
                ));
            }
            match &output.decision {
                Decision::Rewrite { patch } if interceptable => {
                    // `caller` / `parent_call` / `source` are kernel-owned: a
                    // rewrite may not forge who issued a call or posted. Restore
                    // them verbatim, then re-extract the fields so guards judge
                    // the final, unforgeable payload.
                    let caller = ev.payload.get("caller").cloned();
                    let parent = ev.payload.get("parent_call").cloned();
                    let source = ev
                        .payload
                        .get("source")
                        .cloned()
                        .filter(|_| ev.kind == kind::POST);
                    json_patch::merge(&mut ev.payload, patch);
                    restore_kernel_key(&mut ev.payload, "caller", caller);
                    restore_kernel_key(&mut ev.payload, "parent_call", parent);
                    restore_kernel_key(&mut ev.payload, "source", source);
                    ev.fields = fields::extract_fields(&ev.kind, &ev.payload, &state);
                    last_rewrite_at = calls;
                }
                // Blocks are buffered in the verdict; decided in phase 2.
                Decision::Block { .. } => {}
                // block/rewrite on non-interceptable events: ignored
                _ => {}
            }
            verdict[idx] = Some(output);
        }

        // Phase 2: with the fields final, re-check guards whose verdict was
        // returned before the last rewrite. Only their `block` is honoured —
        // their effects already applied in phase 1, and the fields are
        // frozen after the rewrite phase. A stale verdict whose predicate no
        // longer matches the final payload is dropped.
        if interceptable && last_rewrite_at > 0 {
            for idx in 0..hooks.len() {
                if !called_at[idx].is_some_and(|at| at < last_rewrite_at) {
                    continue;
                }
                let h = &hooks[idx];
                if !h.pred.matches(&ev.fields) {
                    verdict[idx] = None;
                    continue;
                }
                let fresh = self.call_hook(h, ev, true, hook_timeout).await;
                self.push_hook_fired(ev, &h.plugin_name, &h.spec.id, &fresh);
                verdict[idx] = Some(HookOutput {
                    effects: vec![],
                    decision: match fresh.decision {
                        Decision::Block { reason } => Decision::Block { reason },
                        _ => Decision::Pass,
                    },
                });
            }
        }

        // Decide: the first (highest-priority) buffered block wins.
        if interceptable {
            for v in verdict.iter().flatten() {
                if let Decision::Block { reason } = &v.decision {
                    return Ok(Some(reason.clone()));
                }
            }
        }
        Ok(None)
    }

    fn push_hook_fired(&self, ev: &Event, plugin: &str, hook_id: &str, output: &HookOutput) {
        let mut params = json!({
            "branch": ev.branch,
            "event": ev.kind,
            "plugin": plugin,
            "hook_id": hook_id,
            "effects": output.effects,
            "decision": output.decision,
        });
        // Internal `code` calls carry their enclosing call id so the UI can
        // hang the hook trace off the internal call row.
        if let Some(parent) = ev.payload.get("parent_call").and_then(Value::as_str) {
            params["parent_call"] = json!(parent);
        }
        self.push("hook.fired", params);
    }

    /// Flush queued notes/markers at a safe point.
    pub(crate) async fn flush_queue(self: &Arc<Self>, branch: &BranchId) -> Result<()> {
        let queued = {
            self.inner
                .lock()
                .unwrap()
                .queue
                .remove(branch)
                .unwrap_or_default()
        };
        for q in queued {
            self.append_body(branch, q.body, Some(q.id)).await?;
        }
        // Touch the session clock so recent sessions sort correctly.
        self.store.touch_session(&self.session_of(branch)?, now())?;
        Ok(())
    }
}

/// Fail-closed: an intercept hook that errors or times out on an interceptable
/// event becomes a synthetic block instead of a no-op; on other events it just
/// means "no effects". Either way, logged.
fn fail_closed(interceptable: bool, who: &str, what: String) -> HookOutput {
    tracing::warn!("intercept hook {who} {what}");
    if interceptable {
        HookOutput {
            effects: vec![],
            decision: Decision::Block {
                reason: format!("hook {who} {what}"),
            },
        }
    } else {
        HookOutput::default()
    }
}

/// Undo a rewrite's effect on a kernel-owned payload key: restore the
/// pre-patch value, or remove the key when it was absent. A patch that
/// replaced the payload with a non-object is coerced back to an object when a
/// key has to be restored.
fn restore_kernel_key(payload: &mut Value, key: &str, old: Option<Value>) {
    match old {
        Some(v) => {
            if !payload.is_object() {
                *payload = Value::Object(serde_json::Map::new());
            }
            if let Some(map) = payload.as_object_mut() {
                map.insert(key.to_owned(), v);
            }
        }
        None => {
            if let Some(map) = payload.as_object_mut() {
                map.remove(key);
            }
        }
    }
}
