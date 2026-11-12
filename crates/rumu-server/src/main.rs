//! The `rumu` binary: axum WebSocket server + static frontend.

mod settings;
mod ws;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rumu_blob::Blobs;
use rumu_core::Plugin;
use rumu_kernel::{Kernel, Llm, LlmResponse, Script, ScriptedLlm};
use rumu_llm::GenaiLlm;
use rumu_store::Store;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// How often unreferenced blobs are collected.
const BLOB_GC_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
/// How long an unreferenced blob survives: an upload stays unreferenced until
/// the node carrying it is written, and the grace covers that gap.
const BLOB_GC_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

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

    let store = Arc::new(Store::open(&home.join("sessions"))?);
    let blobs = Blobs::in_home(&home)?;
    let config = rumu_plugins::load_config(&home)?;
    // A home without a registry starts with the current directory as its
    // one workspace, in a project of its own.
    let fresh = !home.join(rumu_plugins::WORKSPACES_FILE).exists();
    let workspaces = rumu_plugins::load_workspaces(&home)?;
    let names: Vec<String> = workspaces.list().into_iter().map(|w| w.name).collect();
    info!(workspaces = %names.join(", "), "workspaces loaded");
    let decks = rumu_cards::Decks::load(&home)?;
    if fresh {
        rumu_plugins::seed_project(&home, &decks, &workspaces)?;
    }
    let mcp = rumu_plugins::McpHub::new(&home);
    let plugins = load_plugins(
        &home,
        &config,
        decks.clone(),
        workspaces.clone(),
        mcp.clone(),
    )
    .await?;
    let genai = Arc::new(GenaiLlm::default().with_blobs(blobs.clone()));
    let settings = Arc::new(settings::Settings::load(&home, genai.clone(), mcp.clone())?);
    let llm = load_llm(genai)?;
    let kernel = Kernel::new(store.clone(), llm, plugins, workspaces.clone())?;
    settings::spawn_mcp_updates(&settings, &kernel, &mcp);
    spawn_blob_gc(store, blobs.clone());
    let stale = settings.stale_lists();
    settings::spawn_refresh(&settings, &kernel, settings.catalog_stale(), stale);

    let app = ws::router(ws::AppState {
        kernel,
        decks,
        home: home.clone(),
        workspaces,
        blobs,
        settings,
    });
    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(home = %home.display(), %addr, "listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Collect blobs no session references: at startup, then every
/// [`BLOB_GC_INTERVAL`]. The store scan and the directory walk are blocking,
/// so the whole pass runs off the async executor.
fn spawn_blob_gc(store: Arc<Store>, blobs: Blobs) {
    tokio::spawn(async move {
        loop {
            let store = store.clone();
            let blobs = blobs.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
                let live = store.referenced_blobs()?;
                Ok(blobs.gc(&live, BLOB_GC_GRACE)?)
            })
            .await
            .expect("blob gc task panicked");
            match result {
                Ok(n) => info!(removed = n, "blob gc pass complete"),
                Err(e) => tracing::warn!(error = %e, "blob gc failed"),
            }
            tokio::time::sleep(BLOB_GC_INTERVAL).await;
        }
    });
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
/// it is `genai`, configured from `settings.yaml`.
fn load_llm(genai: Arc<GenaiLlm>) -> Result<Arc<dyn Llm>> {
    let Some(spec) = std::env::var("RUMU_LLM").ok() else {
        return Ok(genai);
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
    decks: rumu_cards::Decks,
    workspaces: rumu_core::Workspaces,
    mcp: Arc<rumu_plugins::McpHub>,
) -> Result<Vec<Arc<dyn Plugin>>> {
    let plugins = rumu_plugins::load_all(
        home,
        config,
        decks,
        workspaces,
        mcp,
        &[rumu_kernel::agent_tool()],
    )
    .await?;
    let names: Vec<_> = plugins.iter().map(|p| p.manifest().name).collect();
    info!(plugins = %names.join(", "), "plugins loaded");
    Ok(plugins)
}
