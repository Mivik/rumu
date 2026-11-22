//! The `rumu` binary: axum WebSocket server + static frontend, or with
//! `acp` an Agent Client Protocol agent on stdio.

mod acp;
mod auth;
mod boot;
mod settings;
mod window;
mod ws;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use anyhow::{Result, bail};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let home = boot::home()?;
    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("acp") => return acp::run(&home).await,
        Some(other) => bail!("unknown command `{other}` (usage: rumu [acp])"),
    }
    let token = auth::Token::load(&home)?;
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

    let booted = boot::boot(&home).await?;
    let app = ws::router(booted.state.clone(), token.clone());
    let addr = SocketAddr::new(host, port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    info!(home = %home.display(), %addr, "listening on http://{addr}");
    let browser_host = if host.is_unspecified() {
        if host.is_ipv6() {
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        }
    } else {
        host
    };
    let browser_addr = SocketAddr::new(browser_host, addr.port());
    info!("Open http://{browser_addr}/#token={}", token.value());
    axum::serve(listener, app).await?;
    Ok(())
}

/// Initialize logging; `RUST_LOG` overrides the default `info` level.
/// Colors only on a terminal: `rumu acp`'s stderr usually goes to a log.
fn init_tracing() {
    use std::io::IsTerminal;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
}
