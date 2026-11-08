//! Which protocols an address serves: candidate endpoints are derived from
//! it, and each is asked for its chat route with an empty request. A route
//! that exists reads the request and refuses it as invalid (a JSON 400 or
//! 422); a missing one is a 404 or 405, or not JSON at all. Nothing reaches
//! a model. Many services check the key before routing, so a refused key
//! (401, 403) tells nothing about the route.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::future::join_all;
use rumu_core::Protocol;

use crate::api_root;

/// What [`probe`] found.
#[derive(Debug, Default, serde::Serialize)]
pub struct Probe {
    /// For each protocol `url` serves, the first of its candidates that
    /// answers.
    pub endpoints: BTreeMap<Protocol, String>,
    /// Some candidate refused the key.
    pub denied: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Answer {
    Serves,
    Denied,
    Absent,
}

/// The endpoints `url` serves, asked with `key`.
pub async fn probe(url: &str, key: &str) -> Probe {
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
    else {
        return Probe::default();
    };
    let tries: Vec<(Protocol, String)> = Protocol::ALL
        .into_iter()
        .flat_map(|p| candidates(p, url).into_iter().map(move |c| (p, c)))
        .collect();
    let answers = join_all(tries.iter().map(|(p, c)| answers(&http, *p, c, key))).await;
    let mut found = Probe {
        denied: answers.contains(&Answer::Denied),
        ..Probe::default()
    };
    for ((p, c), a) in tries.into_iter().zip(answers) {
        if a == Answer::Serves {
            found.endpoints.entry(p).or_insert(c);
        }
    }
    found
}

/// Path endings that name an endpoint rather than an API root, as pasted
/// from a request.
const ENDPOINT_TAILS: &[&str] = &[
    "/chat/completions",
    "/completions",
    "/responses",
    "/messages",
    "/embeddings",
    "/models",
];

/// What lies under each level of an address, per protocol, besides the
/// level itself (Anthropic roots are given without `/v1`).
fn layouts(protocol: Protocol) -> &'static [&'static str] {
    match protocol {
        Protocol::Anthropic => &["", "/anthropic", "/api", "/api/anthropic"],
        Protocol::Openai | Protocol::OpenaiResponses => {
            &["/v1", "", "/api/v1", "/openai/v1", "/v1beta/openai"]
        }
        Protocol::Gemini => &["", "/v1beta"],
    }
}

/// `input` as scheme, host and path segments: a missing scheme is http for
/// a local or dotless host and https otherwise; the query, a pasted
/// endpoint (`…/chat/completions`, `…/models/x:generateContent`) and
/// trailing slashes go.
fn normalize(input: &str) -> Option<(String, Vec<String>)> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let url = reqwest::Url::parse(input)
        .ok()
        .filter(|u| u.has_host())
        .or_else(|| {
            let host = input.split(['/', ':']).next().unwrap_or_default();
            let local = host == "localhost"
                || !host.contains('.')
                || host.parse::<std::net::Ipv4Addr>().is_ok();
            let scheme = if local { "http" } else { "https" };
            reqwest::Url::parse(&format!("{scheme}://{input}")).ok()
        })?;
    let origin = url.origin().ascii_serialization();
    let mut path = url.path().trim_end_matches('/').to_owned();
    if let Some(i) = path.find("/models/") {
        path.truncate(i);
    }
    while let Some(tail) = ENDPOINT_TAILS.iter().find(|t| path.ends_with(*t)) {
        path.truncate(path.len() - tail.len());
    }
    let segments = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    Some((origin, segments))
}

/// `v1`, `v4`, `v1beta`.
fn is_version(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
}

/// Where `protocol` may live, given one address of the provider, most
/// likely first: a built-in provider's endpoint on that host, then the
/// address and each level above it, with the usual layouts under each.
fn candidates(protocol: Protocol, input: &str) -> Vec<String> {
    let Some((origin, segments)) = normalize(input) else {
        return Vec::new();
    };
    let mut out: Vec<String> = crate::presets::PRESETS
        .iter()
        .flat_map(|p| p.endpoints.iter())
        .filter(|(p, url)| {
            let on_host = *url == origin || url.starts_with(&format!("{origin}/"));
            *p == protocol && on_host
        })
        .map(|(_, url)| (*url).to_owned())
        .collect();
    for depth in (0..=segments.len()).rev() {
        let level = segments[..depth]
            .iter()
            .fold(origin.clone(), |acc, s| format!("{acc}/{s}"));
        // A level that is a version is an API root as it stands.
        if depth > 0 && is_version(&segments[depth - 1]) {
            out.push(level);
            continue;
        }
        for layout in layouts(protocol) {
            out.push(format!("{level}{layout}"));
        }
    }
    let mut seen = Vec::new();
    out.retain(|c| {
        let fresh = !seen.contains(c);
        seen.push(c.clone());
        fresh
    });
    out
}

async fn answers(http: &reqwest::Client, protocol: Protocol, endpoint: &str, key: &str) -> Answer {
    let root = api_root(protocol, endpoint);
    let json = |r: reqwest::RequestBuilder| {
        r.header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{}")
    };
    let req = match protocol {
        Protocol::Anthropic => json(http.post(format!("{root}messages")))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        Protocol::Openai => json(http.post(format!("{root}chat/completions"))).bearer_auth(key),
        Protocol::OpenaiResponses => json(http.post(format!("{root}responses"))).bearer_auth(key),
        Protocol::Gemini => http
            .get(format!("{root}models"))
            .header("x-goog-api-key", key),
    };
    let Ok(resp) = req.send().await else {
        return Answer::Absent;
    };
    let status = resp.status().as_u16();
    let body: Option<serde_json::Value> = match resp.bytes().await {
        Ok(b) => serde_json::from_slice(&b).ok(),
        Err(_) => None,
    };
    match (protocol, status, body) {
        (_, 401 | 403, _) => Answer::Denied,
        // A list of models (Gemini has no chat route to ask without one).
        (Protocol::Gemini, 200, Some(b)) if b["models"].is_array() => Answer::Serves,
        (Protocol::Gemini, ..) => Answer::Absent,
        (_, 200..=299 | 400 | 422, Some(_)) => Answer::Serves,
        _ => Answer::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(protocol: Protocol, input: &str, n: usize) -> Vec<String> {
        candidates(protocol, input).into_iter().take(n).collect()
    }

    #[test]
    fn addresses_are_normalized() {
        let n = |s: &str| normalize(s).map(|(o, p)| format!("{o} {}", p.join("/")));
        assert_eq!(n("gateway:8080").as_deref(), Some("http://gateway:8080 "));
        assert_eq!(
            n(" gw.example/api/v1/ ").as_deref(),
            Some("https://gw.example api/v1")
        );
        assert_eq!(
            n("https://gw.example/v1/chat/completions?x=1").as_deref(),
            Some("https://gw.example v1")
        );
        assert_eq!(
            n("https://g.example/v1beta/models/gemini-x:generateContent").as_deref(),
            Some("https://g.example v1beta")
        );
        assert_eq!(
            n("http://127.0.0.1:8080/v1/messages").as_deref(),
            Some("http://127.0.0.1:8080 v1")
        );
        assert_eq!(n("  "), None);
    }

    #[test]
    fn candidates_cover_the_usual_layouts() {
        // Each level from the address up, with the layouts under it.
        let c = candidates(Protocol::Anthropic, "https://gw.example/api/v1");
        for want in [
            "https://gw.example/api",
            "https://gw.example/api/anthropic",
            "https://gw.example",
        ] {
            assert!(c.contains(&want.to_owned()), "{want} in {c:?}");
        }
        // A versioned level is taken as is; the next is the one above.
        assert_eq!(
            first(Protocol::Openai, "gw.example/api/anthropic/v1/messages", 2),
            [
                "https://gw.example/api/anthropic/v1",
                "https://gw.example/api/anthropic"
            ]
        );
        assert_eq!(
            first(Protocol::Openai, "gateway:8080", 1),
            ["http://gateway:8080/v1"]
        );
        assert!(
            candidates(Protocol::Openai, "gw.example/api/anthropic")
                .contains(&"https://gw.example/api/v1".to_owned())
        );
        assert!(
            candidates(Protocol::Anthropic, "https://open.bigmodel.cn/api/paas/v4")
                .contains(&"https://open.bigmodel.cn/api/anthropic".to_owned())
        );
        // A built-in provider's host: its endpoints first.
        assert_eq!(
            first(Protocol::Anthropic, "api.deepseek.com", 1),
            ["https://api.deepseek.com/anthropic"]
        );
        assert_eq!(
            first(Protocol::Openai, "https://openrouter.ai", 1),
            ["https://openrouter.ai/api/v1"]
        );
        assert!(candidates(Protocol::Openai, " ").is_empty());
    }
}
