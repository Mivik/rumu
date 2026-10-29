//! Live smoke test for a running rumu server.
//!
//! Usage: `cargo run -p rumu-server --example smoke` (optional `RUMU_WS` env,
//! default `ws://127.0.0.1:7878/ws`).
//!
//! Creates a session, prompts twice, prints the assistant replies and the
//! Usage (including cached_tokens) of each turn to verify prompt caching
//! through the proxy.

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .without_time()
        .with_target(false)
        .with_writer(std::io::stdout)
        .init();
    let url = std::env::var("RUMU_WS").unwrap_or_else(|_| "ws://127.0.0.1:7878/ws".to_owned());
    let (ws, _) = connect_async(&url).await?;
    info!("connected to {url}");
    let (mut sink, mut stream) = ws.split();

    // session.create
    let create = request(&mut sink, &mut stream, "session.create", &json!({})).await?;
    let session = create["result"]["session"]["id"]
        .as_str()
        .expect("session id")
        .to_owned();
    let branch = create["result"]["session"]["active_branch"]
        .as_str()
        .expect("active branch")
        .to_owned();
    info!("session {session}, branch {branch}");

    for (i, text) in [
        "Reply with exactly: pong",
        "Reply with exactly: ping (second turn, checking the cache)",
    ]
    .into_iter()
    .enumerate()
    {
        request(
            &mut sink,
            &mut stream,
            "prompt",
            &json!({ "branch": branch, "text": text }),
        )
        .await?;

        // Wait for the turn to finish; count pushes along the way.
        let (mut appended, mut deltas) = (0u32, 0u32);
        loop {
            let msg = next_json(&mut stream).await?;
            if msg.get("id").is_some() {
                continue; // a response, not a push
            }
            match msg["method"].as_str().unwrap_or_default() {
                "node.appended" => appended += 1,
                "llm.delta" => deltas += 1,
                "turn.done" | "turn.error"
                    if msg["params"]["branch"].as_str() == Some(branch.as_str()) =>
                {
                    if msg["method"] == "turn.error" {
                        anyhow::bail!("turn.error: {}", msg["params"]["message"]);
                    }
                    break;
                }
                _ => {}
            }
        }
        info!(
            "turn {} finished: {appended} node.appended, {deltas} llm.delta",
            i + 1
        );

        // Report the last assistant node + usage.
        let tree = request(
            &mut sink,
            &mut stream,
            "session.tree",
            &json!({ "session": session }),
        )
        .await?;
        let nodes = tree["result"]["nodes"].as_array().expect("nodes");
        let last = nodes
            .iter()
            .rev()
            .find(|n| n["body"]["type"] == "assistant")
            .expect("an assistant node");
        let usage = &last["body"]["usage"];
        info!(
            "assistant ({}): {:?}",
            last["body"]["model"].as_str().unwrap_or("?"),
            last["body"]["parts"][0]["text"].as_str().unwrap_or("")
        );
        info!(
            "usage: prompt={} completion={} cached={} cache_creation={}",
            usage["prompt_tokens"],
            usage["completion_tokens"],
            usage["cached_tokens"],
            usage["cache_creation_tokens"]
        );
    }
    info!("smoke test OK");
    Ok(())
}

async fn request(
    sink: &mut (impl SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin),
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    method: &str,
    params: &Value,
) -> anyhow::Result<Value> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    sink.send(Message::text(
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string(),
    ))
    .await?;
    loop {
        let v = next_json(stream).await?;
        if v.get("id").and_then(Value::as_u64) == Some(id) {
            if let Some(err) = v.get("error") {
                anyhow::bail!("`{method}` failed: {err}");
            }
            return Ok(v);
        }
    }
}

async fn next_json(
    stream: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> anyhow::Result<Value> {
    while let Some(msg) = stream.next().await {
        match msg? {
            Message::Text(t) => return Ok(serde_json::from_str(t.as_str())?),
            Message::Close(_) => anyhow::bail!("socket closed"),
            _ => {}
        }
    }
    anyhow::bail!("stream ended")
}
