//! One persistent bearer token, shared by browser cookies and API clients.

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use subtle::ConstantTimeEq;

const COOKIE_NAME: &str = "rumu_token";

#[derive(Clone)]
pub struct Token(Arc<str>);

impl Token {
    pub fn load(home: &Path) -> Result<Self> {
        let path = home.join("token");
        let value = match std::fs::read_to_string(&path) {
            Ok(value) => value,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                let value = format!(
                    "{}{}",
                    uuid::Uuid::new_v4().simple(),
                    uuid::Uuid::new_v4().simple()
                );
                // NamedTempFile is owner-only; publish a complete file without
                // overwriting a token another starting process just created.
                let mut file = tempfile::NamedTempFile::new_in(home)?;
                writeln!(file, "{value}")?;
                file.as_file().sync_all()?;
                match file.persist_noclobber(&path) {
                    Ok(_) => value,
                    Err(e) if e.error.kind() == ErrorKind::AlreadyExists => {
                        std::fs::read_to_string(&path)?
                    }
                    Err(e) => return Err(e.error).context("saving authentication token"),
                }
            }
            Err(e) => return Err(e).context("reading authentication token"),
        };
        let value = value.trim();
        if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("invalid authentication token in {}", path.display());
        }
        Ok(Self(value.into()))
    }

    pub fn value(&self) -> &str {
        &self.0
    }

    fn accepts(&self, headers: &HeaderMap) -> bool {
        // An explicit but invalid bearer must not fall back to a saved cookie.
        let candidate = if let Some(value) = headers.get(AUTHORIZATION) {
            value.to_str().ok().and_then(|s| s.strip_prefix("Bearer "))
        } else {
            headers
                .get_all(COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .find_map(|v| {
                    v.split(';').find_map(|part| {
                        let (name, value) = part.trim().split_once('=')?;
                        (name == COOKIE_NAME).then_some(value)
                    })
                })
        };
        candidate.is_some_and(|s| bool::from(self.0.as_bytes().ct_eq(s.as_bytes())))
    }
}

/// Apply before the static fallback: only application assets are public.
pub fn protect<S: Clone + Send + Sync + 'static>(router: Router<S>, token: Token) -> Router<S> {
    router
        .route(
            "/auth",
            get(|| async { StatusCode::NO_CONTENT }).post(login),
        )
        .route_layer(middleware::from_fn_with_state(token.clone(), authorize))
        .layer(axum::Extension(token))
}

async fn authorize(State(token): State<Token>, request: Request, next: Next) -> Response {
    let mut response = if !token.accepts(request.headers()) {
        (StatusCode::UNAUTHORIZED, "authentication required\n").into_response()
    } else if !origin_allowed(request.headers()) {
        (StatusCode::FORBIDDEN, "cross-origin request refused\n").into_response()
    } else {
        next.run(request).await
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn login(axum::Extension(token): axum::Extension<Token>, headers: HeaderMap) -> Response {
    // Origin is supplied by the browser even behind a TLS-terminating proxy;
    // do not trust arbitrary forwarded headers to decide cookie transport.
    let secure = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<Uri>().ok())
        .is_some_and(|uri| uri.scheme_str() == Some("https"));
    let cookie = format!(
        "{COOKIE_NAME}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age=31536000{}",
        token.value(),
        if secure { "; Secure" } else { "" },
    );
    (StatusCode::NO_CONTENT, [(SET_COOKIE, cookie)]).into_response()
}

/// A token is always required, including for clients without Origin. Domain
/// deployments are safe to allow now that DNS rebinding alone grants nothing.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(ORIGIN) else {
        return true;
    };
    let Some(uri) = origin.to_str().ok().and_then(|s| s.parse::<Uri>().ok()) else {
        return false;
    };
    if !matches!(uri.scheme_str(), Some("http" | "https")) {
        return false;
    }
    let Some(authority) = uri.authority() else {
        return false;
    };
    let host = headers
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default();
    authority.as_str().eq_ignore_ascii_case(host)
        || (cfg!(debug_assertions)
            && matches!(authority.as_str(), "localhost:5173" | "127.0.0.1:5173"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_token_persists_and_is_private() {
        let home = tempfile::tempdir().unwrap();
        let first = Token::load(home.path()).unwrap();
        assert_eq!(first.value().len(), 64);
        assert_eq!(first.value(), Token::load(home.path()).unwrap().value());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(home.path().join("token"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::write(home.path().join("token"), "broken").unwrap();
        assert!(Token::load(home.path()).is_err());
    }

    #[test]
    fn auth_headers_and_origin() {
        let token = Token("a".repeat(64).into());
        let mut headers = HeaderMap::new();
        assert!(!token.accepts(&headers));
        headers.insert(
            COOKIE,
            format!("other=1; {COOKIE_NAME}={}", token.value())
                .parse()
                .unwrap(),
        );
        assert!(token.accepts(&headers));
        headers.insert(AUTHORIZATION, "Bearer wrong".parse().unwrap());
        assert!(!token.accepts(&headers));
        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", token.value()).parse().unwrap(),
        );
        assert!(token.accepts(&headers));
        assert!(origin_allowed(&headers));
        headers.insert(HOST, "app.example".parse().unwrap());
        headers.insert(ORIGIN, "https://app.example".parse().unwrap());
        assert!(origin_allowed(&headers));
        for origin in [
            "https://evil.example",
            "null",
            "https://app.example.evil.example",
            "ftp://app.example",
        ] {
            headers.insert(ORIGIN, origin.parse().unwrap());
            assert!(!origin_allowed(&headers));
        }
    }

    #[tokio::test]
    async fn auth_routes_require_credentials_and_issue_cookie() {
        let token = Token("a".repeat(64).into());
        let router = protect(
            Router::new()
                .route("/ws", get(|| async { "protected" }))
                .route("/blobs", axum::routing::post(|| async { "protected" }))
                .route("/blobs/{hash}", get(|| async { "protected" })),
            token.clone(),
        )
        .fallback(|| async { "public" });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");
        assert_eq!(
            client.get(&base).send().await.unwrap().status(),
            StatusCode::OK
        );
        for (method, path) in [
            ("GET", "/auth"),
            ("POST", "/auth"),
            ("GET", "/ws"),
            ("POST", "/blobs"),
            ("GET", "/blobs/hash"),
        ] {
            for credential in [None, Some("wrong"), Some(token.value())] {
                let mut req = client.request(method.parse().unwrap(), format!("{base}{path}"));
                if let Some(value) = credential {
                    req = req.bearer_auth(value);
                }
                let response = req.send().await.unwrap();
                assert_eq!(
                    response.status().is_success(),
                    credential == Some(token.value()),
                    "{method} {path}"
                );
                if credential != Some(token.value()) {
                    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                }
                assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
            }
        }
        let response = client
            .post(format!("{base}/auth"))
            .bearer_auth(token.value())
            .header(ORIGIN, format!("https://{addr}"))
            .send()
            .await
            .unwrap();
        let cookie = response.headers()[SET_COOKIE].to_str().unwrap();
        for flag in [
            "HttpOnly",
            "SameSite=Strict",
            "Max-Age=31536000",
            "Secure",
            "Path=/",
        ] {
            assert!(cookie.contains(flag));
        }
        let response = client
            .get(format!("{base}/blobs/hash"))
            .header(COOKIE, cookie.split(';').next().unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = client
            .post(format!("{base}/auth"))
            .bearer_auth(token.value())
            .header(ORIGIN, "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        server.abort();
    }
}
