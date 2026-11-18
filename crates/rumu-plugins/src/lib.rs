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
//! [`load_all`].

use std::{path::Path, sync::Arc};

use rumu_core::Plugin;

mod approve;
mod ask;
mod code;
mod config;
mod decks;
mod error;
mod mcp;
mod memory;
mod skills;
mod stdio;
mod workspace;
mod workspaces;

pub use ask::{AskPlugin, Assistant, assistant_of};
pub use code::CodePlugin;
pub use config::{Config, load_config};
pub use decks::{AssistantConfig, ProjectChange, save_assistant, update_project};
pub use error::{Error, Result};
pub use mcp::{McpHub, McpPlugin};
pub use memory::MemoryPlugin;
pub use skills::{SKILLS_DIR, Skills, SkillsPlugin};
pub use stdio::{StdioError, StdioPlugin};
pub use workspace::{
    BackendError, RevertAction, RevertFile, RevertPlan, WorkspacePlugin, revert_files, revert_note,
    revert_plan, snapshot_file,
};
pub use workspaces::{
    PROJECTS_FILE, Project, WORKSPACES_FILE, create_project, cwd_workspace_name, drop_project,
    load_workspaces, project_of, projects, remove_workspace, seed_project, set_project_workspaces,
    upsert_workspace,
};

#[cfg(test)]
pub(crate) mod testutil;

/// Load every plugin: memory, workspace, ask, mcp, skills, then the external stdio plugins
/// registered in `<home>/config.yaml` (each spawned and `initialize`d). The
/// caller supplies the parsed [`Config`], the deck and workspace registries
/// and the kernel's built-in tools so the `code` plugin's API stub covers
/// them.
pub async fn load_all(
    home: &Path,
    config: &Config,
    decks: rumu_cards::Decks,
    workspaces: rumu_core::Workspaces,
    mcp: Arc<McpHub>,
    skills: Arc<Skills>,
    builtin_tools: &[rumu_core::ToolSpec],
) -> Result<Vec<Arc<dyn Plugin>>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = vec![
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
    for spec in config.external_specs(home)? {
        let plugin = StdioPlugin::spawn(&spec.name, &spec.argv, &spec.cwd)
            .await
            .map_err(|source| Error::ExternalPlugin {
                name: spec.name.clone(),
                source,
            })?;
        plugins.push(Arc::new(plugin));
    }
    // `code` is built last: its API stub covers every other plugin's tools
    // (stdio included) plus the kernel's built-in tools passed by the caller.
    // It exposes only `code` itself, so there is no cycle.
    let mut all_tools: Vec<rumu_core::ToolSpec> = plugins
        .iter()
        .flat_map(|p| p.manifest().tools)
        .filter(|t| t.name != rumu_core::DELEGATE_TOOL)
        .collect();
    all_tools.extend_from_slice(builtin_tools);
    plugins.push(Arc::new(CodePlugin::new(&all_tools)));
    Ok(plugins)
}
