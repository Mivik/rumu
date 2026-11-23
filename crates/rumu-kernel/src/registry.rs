//! The plugin registry: what the loaded plugins contribute (tools, hooks,
//! system prompt), built from one plugin list and swapped whole by
//! [`Kernel::set_plugins`]. A turn takes one snapshot and keeps it, so a
//! swap never mixes two plugin sets inside one model call.

use std::collections::HashMap;
use std::sync::Arc;

use rumu_core::{HookSpec, Plugin, ToolSpec, Workspace};
use serde_json::{Value, json};

use crate::predicate::Pred;
use crate::{BASE_SYSTEM, Error, Kernel, KernelInbox, TASK_INSTRUCTIONS, agent_tool};

/// A hook registered with the kernel: its spec plus the owning plugin. The
/// matcher is compiled here; dispatch evaluates the compiled form.
pub(crate) struct RegisteredHook {
    pub plugin: Arc<dyn Plugin>,
    pub plugin_name: String,
    pub spec: HookSpec,
    pub pred: Pred,
}

/// A tool's owner.
#[derive(Clone)]
pub(crate) struct Route {
    pub plugin: Arc<dyn Plugin>,
    pub name: String,
}

pub(crate) struct Registry {
    pub plugins: Vec<(String, Arc<dyn Plugin>)>,
    /// All hooks, sorted by (priority, registration order).
    pub hooks: Vec<RegisteredHook>,
    /// Tool name → owning plugin.
    pub routes: HashMap<String, Route>,
    /// System prompt: base + plugin instructions (registration order).
    pub system: String,
    /// All plugin tools, sorted by name.
    pub tools: Vec<ToolSpec>,
}

/// A plugin left out of the registry, and why.
#[derive(Debug, Clone)]
pub struct Rejected {
    pub plugin: String,
    pub error: String,
}

impl Registry {
    /// Build the registry of `plugins`. A plugin whose hook matcher does not
    /// compile is left out whole (its tools too) and reported.
    pub fn build(plugins: Vec<Arc<dyn Plugin>>, workspaces: &[Workspace]) -> (Self, Vec<Rejected>) {
        let mut system = BASE_SYSTEM.to_owned();
        let mut tools = Vec::new();
        let mut routes = HashMap::new();
        let mut hooks = Vec::new();
        let mut kept = Vec::with_capacity(plugins.len());
        let mut rejected = Vec::new();

        'plugins: for p in plugins {
            let m = p.manifest();
            let mut own = Vec::with_capacity(m.hooks.len());
            for h in m.hooks {
                match Pred::compile_with_workspaces(&h.matcher, workspaces) {
                    Ok(pred) => own.push(RegisteredHook {
                        plugin: p.clone(),
                        plugin_name: m.name.clone(),
                        spec: h,
                        pred,
                    }),
                    Err(source) => {
                        let error = Error::Hook {
                            plugin: m.name.clone(),
                            hook: h.id.clone(),
                            source,
                        };
                        tracing::warn!("plugin `{}` left out: {error}", m.name);
                        rejected.push(Rejected {
                            plugin: m.name.clone(),
                            error: error.to_string(),
                        });
                        continue 'plugins;
                    }
                }
            }
            hooks.extend(own);
            if let Some(instr) = &m.instructions {
                system.push_str("\n\n");
                system.push_str(instr);
            }
            for t in m.tools {
                if routes.contains_key(&t.name) {
                    tracing::warn!(
                        tool = %t.name,
                        "duplicate tool registration (first registration wins)"
                    );
                } else {
                    routes.insert(
                        t.name.clone(),
                        Route {
                            plugin: p.clone(),
                            name: m.name.clone(),
                        },
                    );
                }
                tools.push(t);
            }
            kept.push((m.name, p));
        }
        // The kernel owns `agent`; it is not routed to a plugin.
        // `delegate` is `agent`'s half for another assistant: routed,
        // never listed.
        system.push_str("\n\n");
        system.push_str(TASK_INSTRUCTIONS);
        tools.retain(|t| t.name != "agent");
        tools.push(agent_tool());
        // Advertise `background` only on tools that may run as a task.
        for t in &mut tools {
            if t.background
                && let Some(props) = t
                    .schema
                    .get_mut("properties")
                    .and_then(Value::as_object_mut)
            {
                props.insert(
                    "background".into(),
                    json!({
                        "type": "boolean",
                        "description": "run in the background; you get a task id now and a notification when it finishes"
                    }),
                );
            }
        }
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools.dedup_by(|a, b| a.name == b.name);
        hooks.sort_by_key(|h| h.spec.priority); // stable: registration order kept
        let registry = Self {
            plugins: kept,
            hooks,
            routes,
            system,
            tools,
        };
        (registry, rejected)
    }
}

impl Kernel {
    /// The current registry. Take it once per use: a later call may see
    /// another plugin set.
    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.registry.read().unwrap().clone()
    }

    /// Give each plugin of `registry` its channel, bound to its name.
    pub(crate) fn attach(self: &Arc<Self>, registry: &Registry) {
        for (name, p) in &registry.plugins {
            p.attach(Arc::new(KernelInbox {
                kernel: self.clone(),
                source: name.clone(),
            }));
        }
    }

    /// Replace the plugin set: the next model call and hook dispatch see
    /// the new tools, hooks and instructions; calls in flight finish on the
    /// old set. Returns the plugins left out.
    pub fn set_plugins(self: &Arc<Self>, plugins: Vec<Arc<dyn Plugin>>) -> Vec<Rejected> {
        let (registry, rejected) = Registry::build(plugins, &self.workspaces.list());
        self.attach(&registry);
        *self.registry.write().unwrap() = Arc::new(registry);
        rejected
    }
}
