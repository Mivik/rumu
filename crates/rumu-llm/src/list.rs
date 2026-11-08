//! Listing a provider's models from its endpoints' own list APIs.

use std::collections::BTreeMap;
use std::time::Duration;

use rumu_core::Protocol;
use serde_json::Value;

use crate::api_root;

/// Every model id the endpoints list, merged and sorted. An endpoint that
/// fails is skipped; only when all fail is it an error (the first one's).
pub async fn list_models(
    endpoints: &BTreeMap<Protocol, String>,
    key: &str,
) -> Result<Vec<String>, String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let mut ids: Vec<String> = Vec::new();
    let mut error: Option<String> = None;
    let mut seen: Vec<String> = Vec::new();
    for (&protocol, url) in endpoints {
        let root = api_root(protocol, url);
        // Chat completions and Responses share their list.
        if seen.contains(&root) {
            continue;
        }
        seen.push(root.clone());
        match list_one(&http, protocol, &root, key).await {
            Ok(found) => ids.extend(found),
            Err(e) => {
                error.get_or_insert(format!("{root}: {e}"));
            }
        }
    }
    if ids.is_empty()
        && let Some(e) = error
    {
        return Err(e);
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

async fn list_one(
    http: &reqwest::Client,
    protocol: Protocol,
    root: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let req = match protocol {
        Protocol::Anthropic => http
            .get(format!("{root}models?limit=1000"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        Protocol::Openai | Protocol::OpenaiResponses => {
            http.get(format!("{root}models")).bearer_auth(key)
        }
        Protocol::Gemini => http
            .get(format!("{root}models?pageSize=1000"))
            .header("x-goog-api-key", key),
    };
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        return Err(status.to_string());
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(parse(protocol, &body))
}

/// The ids in a list response. Gemini's are `models/…`, and include models
/// that cannot chat.
fn parse(protocol: Protocol, body: &Value) -> Vec<String> {
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    match protocol {
        Protocol::Gemini => body["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                m["supportedGenerationMethods"]
                    .as_array()
                    .is_some_and(|ms| ms.iter().any(|x| x == "generateContent"))
            })
            .filter_map(|m| str_of(m, "name"))
            .map(|n| n.strip_prefix("models/").map(str::to_owned).unwrap_or(n))
            .collect(),
        _ => body["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| str_of(m, "id"))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_list_responses() {
        let openai = json!({"data": [{"id": "gpt-x"}, {"id": "claude-y"}]});
        assert_eq!(parse(Protocol::Openai, &openai), ["gpt-x", "claude-y"]);
        let gemini = json!({"models": [
            {"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]},
        ]});
        assert_eq!(parse(Protocol::Gemini, &gemini), ["gemini-x"]);
    }
}
