# ADR-0015: Embed the web UI in the binary and serve it on the API's origin

- Status: Proposed
- Date: 2026-11-13
- Deciders: Max Nölp (owner), Claude (agent, draft)

## Context

M6 adds the React PWA ([ADR-0009](0009-frontend-react-tauri.md)). Self-hosters run one image plus Postgres
([ADR-0002](0002-rust-axum-single-binary.md), [ADR-0014](0014-release-and-versioning.md)). The PWA authenticates with the
`hm_session` cookie (`SameSite=Lax`) and the derived CSRF header ([ADR-0013](0013-sessions-csrf-and-key-rotation.md)),
which work without extra configuration only when the UI and `/api/v1` share one origin. CI refuses OpenSSL and the
server crate must still build with plain `cargo build` (contributors without Node, `cargo test` in CI).

## Decision

- The UI lives in `web/` (Vite, TypeScript, React) and builds to `web/dist`.
- `hm-server` embeds `web/dist` with `rust-embed` behind the cargo feature **`web-ui`**. The Docker image and therefore
  every release enable it; without the feature, `/` serves a short placeholder page explaining how to build the UI.
- Axum serves the files as the router's **fallback** (not as routes, so the OpenAPI document is unchanged): an existing
  file is served with its MIME type; any other `GET`/`HEAD` path outside `/api/` gets `index.html` (client-side
  routing); paths under `/api/` keep the JSON 404.
- Every static response carries a strict Content Security Policy (`default-src 'self'`, no inline scripts,
  `connect-src 'self'`, `frame-ancestors 'none'`, `base-uri 'none'`, `form-action 'self'`), `X-Content-Type-Options:
  nosniff` and `Referrer-Policy: same-origin`. Files under `/assets/` (content-hashed by Vite) are cached for a year as
  `immutable`; everything else (`index.html`, `sw.js`, `manifest.webmanifest`, icons) is `no-cache`.
- The UI loads nothing from third-party hosts: fonts and icons are bundled.

## Consequences

- **Pro:** one artefact, one process, one port; the UI version always matches the API; cookie auth and CSRF work
  without CORS or proxy rules; self-hosters configure nothing new.
- **Pro:** a strict CSP is possible because we control every asset.
- **Con:** a UI change needs a new binary/image; the Docker build gains a Node stage (a few seconds with cache).
- **Con:** two build tools in one repository; `cargo build --features web-ui` needs `web/dist` built first.
- Rules out: a separate static-file container (nginx), loading the UI from a directory at runtime, CDN-hosted fonts.
