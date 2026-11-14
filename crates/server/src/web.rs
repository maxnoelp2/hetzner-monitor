//! The web UI (ADR-0015): the router's fallback serves the React PWA from `web/dist`, embedded into the binary
//! with the cargo feature `web-ui`. Without the feature, every page is a short placeholder that explains how to
//! build the UI.
//!
//! Rules of the fallback (it only sees requests no API route matched):
//! - `/api` and `/api/…`: the JSON 404 (`not_found`) for every method, so API clients never get HTML.
//! - Other `GET`/`HEAD`: an embedded file if the path names one, otherwise `index.html` (client-side routing).
//!   Paths with empty, `.` or `..` segments or backslashes never name a file; they get `index.html` as well.
//! - Exception: a path under `/assets/` that names no embedded file gets the JSON 404, not `index.html`.
//!   These URLs are content-hashed and cached as immutable (also cache-first by the service worker), so a
//!   client on an older build must not store HTML under a script or font URL.
//! - Other methods outside `/api`: the JSON 404 too. There is no resource to report a 405 for, and the API
//!   already answers unknown routes this way.
//!
//! Every response of the fallback carries a strict Content Security Policy, `nosniff` and a same-origin
//! referrer policy. Embedded files under `/assets/` are content-hashed by Vite and cached for a year;
//! everything else (`index.html`, `sw.js`, the manifest, icons, the placeholder) is `no-cache`.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;

/// The Content Security Policy of every page and file the fallback serves: same origin only, no inline code.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
    img-src 'self'; font-src 'self'; connect-src 'self'; manifest-src 'self'; worker-src 'self'; \
    object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// `Cache-Control` of embedded files under `/assets/` (their names contain a content hash).
pub const CACHE_IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// `Cache-Control` of everything else: the browser revalidates before each use.
pub const CACHE_REVALIDATE: &str = "no-cache";

/// The page served for every non-API `GET` when the binary was built without the `web-ui` feature (or the
/// feature was on but `web/dist` did not exist at build time). No inline script or style, so the CSP holds.
pub const PLACEHOLDER_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>hetzner-monitor</title>
</head>
<body>
<h1>hetzner-monitor</h1>
<p>The web UI is not built into this binary. The API is available under <a href="/api/v1/openapi.json">/api/v1</a>.</p>
<p>To include the web UI, build it first and then the server with the <code>web-ui</code> feature:</p>
<pre>cd web
npm ci
npm run build
cd ..
cargo build --release -p hm-server --features web-ui</pre>
<p>The published Docker image already contains the web UI.</p>
</body>
</html>
"#;

/// The router's fallback (registered in [`crate::http::router`]). See the module docs for the rules.
pub async fn fallback(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    let head = method == Method::HEAD;
    let mut response = if is_api(path) || !(method == Method::GET || head) {
        ApiError::not_found().into_response()
    } else {
        page(path, &headers)
    };
    let cache = if response
        .headers()
        .get(header::CACHE_CONTROL)
        .is_some_and(|v| v == CACHE_IMMUTABLE)
    {
        CACHE_IMMUTABLE
    } else {
        CACHE_REVALIDATE
    };
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if head {
        // Keep the headers (including Content-Length) and drop the body.
        let (parts, _) = response.into_parts();
        response = Response::from_parts(parts, Body::empty());
    }
    response
}

/// A path under `/assets/` (Vite's content-hashed build output).
fn is_asset(path: &str) -> bool {
    path.starts_with("/assets/")
}

/// The answer for a path that names no embedded file: the JSON 404 under `/assets/`, else `page`.
fn not_a_file(path: &str, page: impl FnOnce() -> Response) -> Response {
    if is_asset(path) {
        ApiError::not_found().into_response()
    } else {
        page()
    }
}

/// `/api` itself or anything below it (but not `/apiary`).
fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The embedded-file key for a request path, or `None` if the path cannot name a file (root, empty, `.` or
/// `..` segments, backslashes). The path is used as sent (percent-encoded), so encoded dots never match.
#[cfg_attr(not(feature = "web-ui"), allow(dead_code))]
fn file_key(path: &str) -> Option<&str> {
    let key = path.strip_prefix('/')?;
    let valid = !key.is_empty()
        && key
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains('\\'));
    valid.then_some(key)
}

/// A `200 text/html` response with the placeholder page.
fn placeholder() -> Response {
    html_response(Body::from(PLACEHOLDER_HTML), PLACEHOLDER_HTML.len())
}

fn html_response(body: Body, len: usize) -> Response {
    let mut response = Response::new(body);
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    response
}

#[cfg(not(feature = "web-ui"))]
fn page(path: &str, _headers: &HeaderMap) -> Response {
    not_a_file(path, placeholder)
}

#[cfg(feature = "web-ui")]
fn page(path: &str, headers: &HeaderMap) -> Response {
    embedded::page(path, headers)
}

/// Files of `web/dist`. In release builds they are compiled into the binary; in debug builds rust-embed reads
/// them from `web/dist` at runtime. `allow_missing` keeps the build working without `web/dist` (CI runs
/// clippy and tests with all features but does not build the UI); the set is then empty and every page is
/// the placeholder.
#[cfg(feature = "web-ui")]
mod embedded {
    use std::fmt::Write as _;

    use axum::body::Body;
    use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
    use axum::response::Response;
    use rust_embed::{Embed, EmbeddedFile};

    use super::{CACHE_IMMUTABLE, file_key, not_a_file, placeholder};

    #[derive(Embed)]
    #[folder = "../../web/dist"]
    #[allow_missing = true]
    struct Dist;

    pub(super) fn page(path: &str, headers: &HeaderMap) -> Response {
        if let Some(key) = file_key(path)
            && let Some(file) = Dist::get(key)
        {
            let mut response = file_response(&file, headers);
            if key.starts_with("assets/") {
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static(CACHE_IMMUTABLE),
                );
            }
            return response;
        }
        not_a_file(path, || match Dist::get("index.html") {
            Some(index) => file_response(&index, headers),
            None => placeholder(),
        })
    }

    /// The file with its MIME type and a strong `ETag` (its SHA-256); `304` if `If-None-Match` matches.
    fn file_response(file: &EmbeddedFile, headers: &HeaderMap) -> Response {
        let etag = etag(&file.metadata.sha256_hash());
        let not_modified = headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
        let mut response = if not_modified {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::NOT_MODIFIED;
            r
        } else {
            let mut r = Response::new(Body::from(file.data.clone()));
            r.headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(file.data.len()));
            r
        };
        let h = response.headers_mut();
        let mime = file.metadata.mimetype();
        let content_type = if mime.starts_with("text/") {
            HeaderValue::from_str(&format!("{mime}; charset=utf-8"))
        } else {
            HeaderValue::from_str(mime)
        };
        if let Ok(value) = content_type {
            h.insert(header::CONTENT_TYPE, value);
        }
        if let Ok(value) = HeaderValue::from_str(&etag) {
            h.insert(header::ETAG, value);
        }
        response
    }

    fn etag(hash: &[u8; 32]) -> String {
        let mut out = String::with_capacity(66);
        out.push('"');
        for byte in hash {
            let _ = write!(out, "{byte:02x}");
        }
        out.push('"');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{file_key, is_api, is_asset};

    #[test]
    fn api_paths() {
        assert!(is_api("/api"));
        assert!(is_api("/api/"));
        assert!(is_api("/api/v1/nope"));
        assert!(!is_api("/apiary"));
        assert!(!is_api("/"));
        assert!(!is_api("/alerts/api"));
    }

    #[test]
    fn asset_paths() {
        assert!(is_asset("/assets/index-abc.js"));
        assert!(is_asset("/assets/"));
        assert!(!is_asset("/assets"));
        assert!(!is_asset("/alerts/assets/x.js"));
    }

    #[test]
    fn file_keys() {
        assert_eq!(file_key("/index.html"), Some("index.html"));
        assert_eq!(
            file_key("/assets/index-abc.js"),
            Some("assets/index-abc.js")
        );
        assert_eq!(file_key("/"), None);
        assert_eq!(file_key("/assets/"), None);
        assert_eq!(file_key("//etc/passwd"), None);
        assert_eq!(file_key("/../Cargo.toml"), None);
        assert_eq!(file_key("/assets/../../Cargo.toml"), None);
        assert_eq!(file_key("/./index.html"), None);
        assert_eq!(file_key("/assets\\..\\x"), None);
    }
}
