//! The `rumu` binary: axum WebSocket server + static frontend.

mod ws;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rumu_core::Plugin;
use rumu_kernel::{Kernel, Llm, LlmResponse, Script, ScriptedLlm};
use rumu_llm::GenaiLlm;
use rumu_store::Store;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let home: PathBuf = std::env::var("RUMU_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().unwrap_or_default().join(".rumu"));
    std::fs::create_dir_all(&home)
        .with_context(|| format!("creating RUMU_HOME at {}", home.display()))?;
    let port: u16 = std::env::var("RUMU_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(7878);
    // Local-only by default: `bash` runs unsandboxed, so only bind elsewhere
    // deliberately.
    let host: IpAddr = std::env::var("RUMU_HOST")
        .ok()
        .and_then(|h| h.parse().ok())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));

    let store = Store::open(&home.join("sessions"))?;
    let config = rumu_plugins::load_config(&home)?;
    let catalog = config.catalog_or_cwd()?;
    info!(
        workspaces = %catalog
            .workspaces
            .iter()
            .map(|w| format!("{} ({} {})", w.name, w.label(), w.root))
            .collect::<Vec<_>>()
            .join(", "),
        "workspaces loaded"
    );
    let plugins = load_plugins(&home, &config, catalog.clone()).await?;
    let llm = load_llm()?;
    let kernel = Kernel::new(Arc::new(store), llm, plugins, catalog.clone())?;

    let app = ws::router(kernel, home.clone());
    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(home = %home.display(), %addr, "listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Initialize logging; `RUST_LOG` overrides the default `info` level.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// The model backend. `RUMU_LLM=script:<path>` replays a YAML/JSON turn
/// script through [`ScriptedLlm`] (no model endpoint is needed); otherwise
/// the genai adapter reads `RUMU_MODEL` / `ANTHROPIC_*` from the env.
fn load_llm() -> Result<Arc<dyn Llm>> {
    let Some(spec) = std::env::var("RUMU_LLM").ok() else {
        return Ok(Arc::new(GenaiLlm::from_env()?));
    };
    let Some(path) = spec.strip_prefix("script:") else {
        bail!("RUMU_LLM must be `script:<path>` (got `{spec}`)");
    };
    let text = std::fs::read_to_string(path).with_context(|| format!("reading script {path}"))?;
    let script: Script =
        rumu_cards::yaml::from_str(&text).with_context(|| format!("parsing script {path}"))?;
    info!(path, turns = script.turns.len(), "scripted LLM mode");
    Ok(Arc::new(
        ScriptedLlm::looping(script.turns.into_iter().map(LlmResponse::from))
            .with_chunk_delay(Duration::from_millis(35)),
    ))
}

async fn load_plugins(
    home: &Path,
    config: &rumu_plugins::Config,
    catalog: rumu_core::WorkspaceCatalog,
) -> Result<Vec<Arc<dyn Plugin>>> {
    let plugins =
        rumu_plugins::load_all(home, config, catalog, &[rumu_kernel::agent_tool()]).await?;
    let names: Vec<_> = plugins.iter().map(|p| p.manifest().name).collect();
    info!(plugins = %names.join(", "), "plugins loaded");
    Ok(plugins)
}
