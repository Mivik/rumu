//! rumu-plugins: rumu's built-in plugins.
//!
//! - [`MemoryPlugin`] — card surfacing: the catalog, `scope:` related
//!   cards, update notices (decks and cards themselves live in rumu-cards)
//! - [`WorkspacePlugin`] — the `read`/`write`/`edit`/`list`/`bash` file tools
//!   over workspaces (local dirs + ssh), the `@rumu` virtual workspace
//!   (`@rumu:/cards`, `@rumu:/sessions`) and `workspace_use`
//! - [`CodePlugin`] — the `code` tool: model-written JavaScript that composes
//!   the other tools through `Cx::invoke` (hybrid mode)
//! - [`StdioPlugin`] — host side of the external plugin protocol
//!
//! The kernel itself owns the `agent` tool. Entry point for the server:
//! [`load_all`].

use std::{path::Path, sync::Arc};

use rumu_core::Plugin;

mod approve;
mod code;
mod config;
mod error;
mod memory;
mod stdio;
mod workspace;

pub use code::CodePlugin;
pub use config::{Config, load_config};
pub use error::{Error, Result};
pub use memory::MemoryPlugin;
pub use stdio::{StdioError, StdioPlugin};
pub use workspace::{BackendError, WorkspacePlugin};

#[cfg(test)]
pub(crate) mod testutil;

/// Load every plugin: memory, workspace, then the external stdio plugins
/// registered in `<home>/config.yaml` (each spawned and `initialize`d). The
/// caller supplies the parsed [`Config`], the deck registry and the kernel's
/// built-in tools so the `code` plugin's API stub covers them.
pub async fn load_all(
    home: &Path,
    config: &Config,
    catalog: rumu_core::WorkspaceCatalog,
    decks: rumu_cards::Decks,
    builtin_tools: &[rumu_core::ToolSpec],
) -> Result<Vec<Arc<dyn Plugin>>> {
    let mut plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(MemoryPlugin::new(decks.clone())),
        Arc::new(WorkspacePlugin::new(home, catalog, decks)),
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
    let mut all_tools: Vec<rumu_core::ToolSpec> =
        plugins.iter().flat_map(|p| p.manifest().tools).collect();
    all_tools.extend_from_slice(builtin_tools);
    plugins.push(Arc::new(CodePlugin::new(&all_tools)));
    Ok(plugins)
}
