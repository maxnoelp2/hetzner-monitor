//! Integration tests for the web UI fallback (ADR-0015, `src/web.rs`).
//!
//! Without the `web-ui` feature every non-API page is the placeholder. With the feature (`cargo test -p hm-server
//! --features web-ui --test web`) the tests in `embedded` check the files of `web/dist`; they skip with a message
//! when the UI was not built (`npm ci && npm run build` in `web/`).

mod common;

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode};
use hm_server::auth::Scope;
use hm_server::http::{AppState, router};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'self'; \
    connect-src 'self'; manifest-src 'self'; worker-src 'self'; object-src 'none'; base-uri 'none'; \
    form-action 'self'; frame-ancestors 'none'";

fn state(pool: PgPool) -> AppState {
    common::state(pool, "http://127.0.0.1:9")
}

/// Sends a request without a body and returns status, headers and the raw body.
async fn send(
    state: &AppState,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Bytes) {
    let mut req = Request::builder().method(method).uri(uri);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let response = router(state.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, headers, body)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .unwrap_or_else(|| panic!("missing header {name}"))
        .to_str()
        .unwrap()
}

/// The headers every response of the fallback carries.
fn assert_security_headers(headers: &HeaderMap) {
    assert_eq!(header(headers, "content-security-policy"), CSP);
    assert_eq!(header(headers, "x-content-type-options"), "nosniff");
    assert_eq!(header(headers, "referrer-policy"), "same-origin");
}

/// A `200 text/html` page with the security headers and `no-cache`.
fn assert_html_page(status: StatusCode, headers: &HeaderMap, body: &Bytes) {
    assert_eq!(status, StatusCode::OK);
    assert!(
        header(headers, "content-type").starts_with("text/html"),
        "{headers:?}"
    );
    assert_security_headers(headers);
    assert_eq!(header(headers, "cache-control"), "no-cache");
    assert_eq!(
        header(headers, "content-length"),
        body.len().to_string(),
        "content-length matches the body"
    );
    let text = std::str::from_utf8(body).unwrap();
    assert!(text.starts_with("<!doctype html>"), "{text}");
}

fn assert_json_404(status: StatusCode, headers: &HeaderMap, body: &Bytes) {
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(header(headers, "content-type").starts_with("application/json"));
    let value: Value = serde_json::from_slice(body).unwrap();
    assert_eq!(value["error"]["code"], "not_found", "{value}");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn root_serves_an_html_page(pool: PgPool) {
    let state = state(pool);
    let (status, headers, body) = send(&state, "GET", "/", &[]).await;
    assert_html_page(status, &headers, &body);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn client_routes_serve_the_same_page(pool: PgPool) {
    let state = state(pool);
    let (_, _, root) = send(&state, "GET", "/", &[]).await;
    for path in [
        "/alerts/123",
        "/projects",
        "/settings?tab=keys",
        "/../Cargo.toml",
        "/apiary",
    ] {
        let (status, headers, body) = send(&state, "GET", path, &[]).await;
        assert_html_page(status, &headers, &body);
        assert_eq!(body, root, "{path} serves the index page");
    }
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn head_returns_headers_without_body(pool: PgPool) {
    let state = state(pool);
    let (_, get_headers, get_body) = send(&state, "GET", "/alerts", &[]).await;
    let (status, headers, body) = send(&state, "HEAD", "/alerts", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty());
    assert_security_headers(&headers);
    assert_eq!(header(&headers, "cache-control"), "no-cache");
    assert_eq!(
        header(&headers, "content-type"),
        header(&get_headers, "content-type")
    );
    assert_eq!(
        header(&headers, "content-length"),
        get_body.len().to_string()
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn unknown_api_paths_are_json_404(pool: PgPool) {
    let state = state(pool);
    for (method, path) in [
        ("GET", "/api"),
        ("GET", "/api/"),
        ("GET", "/api/v1/does-not-exist"),
        ("GET", "/api/v2/alerts"),
        ("HEAD", "/api/v1/does-not-exist"),
        ("POST", "/api/v1/does-not-exist"),
        ("DELETE", "/api"),
    ] {
        let (status, headers, body) = send(&state, method, path, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {path}");
        assert_security_headers(&headers);
        if method == "HEAD" {
            assert!(body.is_empty());
        } else {
            assert_json_404(status, &headers, &body);
        }
    }
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn other_methods_outside_api_are_json_404(pool: PgPool) {
    let state = state(pool);
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        let (status, headers, body) = send(&state, method, "/alerts/123", &[]).await;
        assert_json_404(status, &headers, &body);
    }
}

/// Hashed asset URLs are cached as immutable, so a missing one must be a real 404, never `index.html`.
#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn missing_assets_are_json_404(pool: PgPool) {
    let state = state(pool);
    for path in [
        "/assets/does-not-exist.js",
        "/assets/old-hash.woff2",
        "/assets/",
        "/assets/%2e%2e/%2e%2e/Cargo.toml",
        "/assets/../index.html",
    ] {
        let (status, headers, body) = send(&state, "GET", path, &[]).await;
        assert_json_404(status, &headers, &body);
        assert_security_headers(&headers);
        assert_eq!(header(&headers, "cache-control"), "no-cache", "{path}");
        let (status, _, body) = send(&state, "HEAD", path, &[]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "HEAD {path}");
        assert!(body.is_empty());
    }
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn api_routes_are_unaffected(pool: PgPool) {
    let state = state(pool.clone());
    let (status, headers, body) = send(&state, "GET", "/healthz", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(header(&headers, "content-type").starts_with("application/json"));
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["status"],
        "ok"
    );
    // The fallback headers belong to the UI only.
    assert!(headers.get("content-security-policy").is_none());

    let (status, headers, _) = send(&state, "GET", "/api/v1/openapi.json", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(header(&headers, "content-type").starts_with("application/json"));

    let (_, key) = common::org_with_key(&pool, "acme", Scope::Read).await;
    let (status, body) = common::call(&state, "GET", "/api/v1/alerts", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = common::call(&state, "GET", "/api/v1/alerts", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
}

/// The placeholder of a binary without the UI: says how to build it, no inline code.
#[cfg(not(feature = "web-ui"))]
#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn placeholder_explains_how_to_build_the_ui(pool: PgPool) {
    let state = state(pool);
    let (_, _, body) = send(&state, "GET", "/", &[]).await;
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("not built into this binary"), "{text}");
    assert!(text.contains("npm ci"));
    assert!(text.contains("npm run build"));
    assert!(text.contains("--features web-ui"));
    assert!(!text.contains("<script"));
    assert!(!text.contains("<style"));
    assert!(!text.contains("style="));
}

/// Files of `web/dist`, embedded with the `web-ui` feature.
#[cfg(feature = "web-ui")]
mod embedded {
    use std::path::{Path, PathBuf};

    use super::*;

    /// `web/dist`, or `None` (with a message) when the UI was not built.
    fn dist() -> Option<PathBuf> {
        let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist");
        if dist.join("index.html").is_file() {
            Some(dist)
        } else {
            eprintln!(
                "skipped: web/dist/index.html does not exist (build the UI: npm ci && npm run build in web/)"
            );
            None
        }
    }

    #[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
    async fn index_html_is_served_for_pages(pool: PgPool) {
        let Some(dist) = dist() else { return };
        let index = std::fs::read(dist.join("index.html")).unwrap();
        let state = state(pool);
        for path in ["/", "/index.html", "/alerts/123"] {
            let (status, headers, body) = send(&state, "GET", path, &[]).await;
            assert_html_page(status, &headers, &body);
            assert_eq!(body.as_ref(), index.as_slice(), "{path}");
            assert!(headers.get("etag").is_some());
        }
    }

    #[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
    async fn hashed_assets_are_immutable(pool: PgPool) {
        let Some(dist) = dist() else { return };
        let asset = std::fs::read_dir(dist.join("assets"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .find(|name| Path::new(name).extension().is_some_and(|ext| ext == "js"))
            .expect("a JavaScript bundle in web/dist/assets");
        let bytes = std::fs::read(dist.join("assets").join(&asset)).unwrap();
        let state = state(pool);
        let (status, headers, body) = send(&state, "GET", &format!("/assets/{asset}"), &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_ref(), bytes.as_slice());
        assert!(header(&headers, "content-type").contains("javascript"));
        assert_eq!(
            header(&headers, "cache-control"),
            "public, max-age=31536000, immutable"
        );
        assert_security_headers(&headers);

        // Revalidation with the ETag: 304 without body, same caching headers.
        let etag = header(&headers, "etag").to_owned();
        let (status, headers, body) = send(
            &state,
            "GET",
            &format!("/assets/{asset}"),
            &[("if-none-match", &etag)],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_MODIFIED);
        assert!(body.is_empty());
        assert_eq!(header(&headers, "etag"), etag);
        assert_eq!(
            header(&headers, "cache-control"),
            "public, max-age=31536000, immutable"
        );
    }

    #[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
    async fn manifest_and_service_worker_are_revalidated(pool: PgPool) {
        let Some(dist) = dist() else { return };
        let state = state(pool);
        if dist.join("manifest.webmanifest").is_file() {
            let (status, headers, _) = send(&state, "GET", "/manifest.webmanifest", &[]).await;
            assert_eq!(status, StatusCode::OK);
            assert!(
                header(&headers, "content-type").starts_with("application/manifest+json"),
                "{headers:?}"
            );
            assert_eq!(header(&headers, "cache-control"), "no-cache");
            assert_security_headers(&headers);
        } else {
            eprintln!("skipped: web/dist/manifest.webmanifest does not exist");
        }
        if dist.join("sw.js").is_file() {
            let (status, headers, _) = send(&state, "GET", "/sw.js", &[]).await;
            assert_eq!(status, StatusCode::OK);
            assert!(header(&headers, "content-type").contains("javascript"));
            assert_eq!(header(&headers, "cache-control"), "no-cache");
        }
    }
}
