//! rumu-plugins: rumu's built-in plugins.
//!
//! - [`MemoryPlugin`] — card surfacing: the catalog, recall, update notices
//!   (decks and cards themselves live in rumu-cards)
//! - [`WorkspacePlugin`] — the `read`/`write`/`edit`/`list`/`bash` file tools
//!   over workspaces (local dirs + ssh), the `@rumu` virtual workspace
//!   (`@rumu:/cards`, `@rumu:/sessions`, …) and `invoke` for its objects'
//!   actions
//! - [`CodePlugin`] — the `code` tool: model-written JavaScript that composes
//!   the other tools through `Cx::invoke` (hybrid mode)
//! - [`McpPlugin`] — the note listing the session's MCP servers of
//!   [`McpHub`] (their tools run through `invoke`)
//! - [`SkillsPlugin`] — the first-call note listing the [`Skills`] enabled
//! - [`StdioPlugin`] — host side of the external plugin protocol
//!
//! The kernel itself owns the `agent` tool. Entry point for the server:
//! [`PluginSet`].

use std::{path::Path, sync::Arc};

use rumu_core::Plugin;

mod approve;
mod ask;
mod code;
mod decks;
mod error;
mod external;
pub mod mcp;
mod memory;
mod skills;
mod stdio;
mod workspace;
mod workspaces;

pub use ask::{AskPlugin, Assistant, assistant_of};
pub use code::CodePlugin;
pub use decks::{AssistantConfig, ProjectChange, create_project, save_assistant, update_project};
pub use error::{Error, Result};
pub use external::ExternalPlugins;
pub use mcp::{McpHub, McpPlugin};
pub use memory::MemoryPlugin;
pub use skills::{SKILLS_DIR, Skills, SkillsPlugin};
pub use stdio::{StdioError, StdioPlugin};
pub use workspace::{
    BackendError, RevertAction, RevertFile, RevertPlan, WorkspacePlugin, read_file, revert_files,
    revert_note, revert_plan, save_page, snapshot_file,
};
pub use workspaces::{
    PROJECTS_FILE, Project, WORKSPACES_FILE, cwd_workspace_name, drop_project, load_workspaces,
    project_of, projects, remove_workspace, seed_project, set_project_workspaces, upsert_workspace,
};

#[cfg(test)]
pub(crate) mod testutil;

/// Every plugin: the built-in ones, made once, and the external ones of
/// `settings.yaml`, which come and go (see [`ExternalPlugins`]).
pub struct PluginSet {
    builtins: Vec<Arc<dyn Plugin>>,
    /// The kernel's own tools, for the `code` plugin's API stub.
    builtin_tools: Vec<rumu_core::ToolSpec>,
    pub external: Arc<ExternalPlugins>,
}

impl PluginSet {
    /// Make the built-in plugins (memory, workspace, ask, mcp, skills); the
    /// external ones start when the settings configure them. `builtin_tools`
    /// are the kernel's own, so the `code` plugin's API stub covers them.
    pub fn new(
        home: &Path,
        decks: rumu_cards::Decks,
        workspaces: rumu_core::Workspaces,
        mcp: Arc<McpHub>,
        skills: Arc<Skills>,
        builtin_tools: &[rumu_core::ToolSpec],
    ) -> Self {
        let builtins: Vec<Arc<dyn Plugin>> = vec![
            Arc::new(MemoryPlugin::new(decks.clone())),
            Arc::new(
                WorkspacePlugin::new(home, decks.clone(), workspaces)
                    .with_mcp(mcp.clone())
                    .with_skills(skills.clone()),
            ),
            Arc::new(AskPlugin::new(decks.clone())),
            Arc::new(McpPlugin::new(mcp, decks.clone())),
            Arc::new(SkillsPlugin::new(skills, decks)),
        ];
        Self {
            builtins,
            builtin_tools: builtin_tools.to_vec(),
            external: ExternalPlugins::new(home),
        }
    }

    /// The plugins to hand the kernel now: the built-in ones, the running
    /// external ones, and `code` last — its API stub covers every other
    /// plugin's tools plus the kernel's, and it exposes only `code` itself,
    /// so there is no cycle.
    pub fn all(&self) -> Vec<Arc<dyn Plugin>> {
        let mut plugins = self.builtins.clone();
        plugins.extend(self.external.plugins());
        let mut tools: Vec<rumu_core::ToolSpec> =
            plugins.iter().flat_map(|p| p.manifest().tools).collect();
        tools.extend_from_slice(&self.builtin_tools);
        plugins.push(Arc::new(CodePlugin::new(&tools)));
        plugins
    }
}
