# Plan: M6 — React PWA with Web Push

- Status: In progress (M6a)
- Milestone: M6 (split into **M6a** web UI and **M6b** Web Push, like M5)
- Owner: agent (Claude), reviewed by the maintainer
- Started: 2026-11-13
- PR: <link once opened> (one PR per half; M6a may be split further if it grows past a reviewable size)

## Goal

A self-hoster installs hetzner-monitor from the README alone, logs into a web UI served by the same binary, connects a
project, sets up channels and rules there, and receives an alert as a push notification on a phone — without curl and
without any service other than the existing image and Postgres.

## Acceptance criteria

### M6a — web UI in the binary, session login

- [ ] `web/` holds a React + TypeScript PWA (Vite). Its production build is embedded into the `hetzner-monitor` binary and
      served from `/` on the same origin as `/api/v1`; unknown non-API paths fall back to `index.html` (client routing),
      `/api/*` never does (an unknown API path stays a JSON 404).
- [ ] Static responses carry a strict CSP (`default-src 'self'`, no inline script, `frame-ancestors 'none'`),
      `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`; hashed assets are `immutable`, `index.html`,
      `sw.js` and `manifest.webmanifest` are `no-cache`. No request leaves for a CDN (fonts and icons are bundled).
- [ ] Login with e-mail and password (`POST /auth/login`), session cookie `hm_session`, CSRF token kept in memory and
      re-read from `GET /me` after a reload; every state-changing call sends `X-CSRF-Token`; a 401 returns to the login
      page; logout; change password (`PUT /me/password`).
- [ ] Screens: alerts (list with state/project filters, detail with events and deliveries, ack), projects (list,
      connect with a token and the "a **Read** token is enough" hint, detail with schedule, budget, cost estimate,
      inventory by kind, sync), rules (list, enable/disable, edit params/severity/selector/channels, create and delete
      custom rules), channels (list, create/edit per type, test, delete), silences (list, create, end), API keys
      (list, create with the key shown once, revoke), account (password).
- [ ] The API client is **generated** from the OpenAPI document (`openapi-typescript`); CI fails if the committed
      generated file is stale (`hetzner-monitor openapi` → regenerate → `git diff --exit-code`).
- [ ] UI follows the owner's design system (tokens, components, voice) re-implemented inside `web/` — dark-first, Geist,
      glass cards, pill buttons, Lucide icons — plus the monitoring pieces it lacks (tables, severity/state badges,
      empty and error states) derived from its tokens. Usable at 375 px width.
- [ ] Installable PWA: `manifest.webmanifest` (name, icons, `display: standalone`, theme colours), a service worker
      (`sw.js`) that caches the app shell only (API responses are never cached).
- [ ] Notification links point to the UI (`<HM_BASE_URL>/alerts/{id}`) instead of `/api/v1/alerts/{id}`.
- [ ] `cargo build` without Node still works: the server crate's `web-ui` feature embeds `web/dist`; without it, `/`
      serves a short placeholder page that says how to build the UI. Docker image and release build enable it.
- [ ] Tests: Rust integration tests for static serving (fallback, `/api` exclusion, headers, cache headers); Vitest +
      Testing Library for the API wrapper (CSRF header, 401 handling) and the login and alert-ack flows.
- [ ] CI: a `web` job (`npm ci`, typecheck, lint, test, build, generated-client check); the Docker build includes a Node
      stage.

### M6b — Web Push

- [ ] VAPID key pair (P-256) generated on first start and stored **envelope-encrypted** in the database (AAD = row id,
      covered by `keys status` / `keys rotate`); no `HM_VAPID_*` key variables. `HM_VAPID_SUBJECT` (`mailto:` or
      `https:`), defaulting to `HM_BASE_URL` when that is `https://`.
- [ ] Push-subscription API, per user and device: `GET /push/vapid-public-key`, `GET /push/subscriptions` (own devices),
      `POST /push/subscriptions {endpoint, keys: {p256dh, auth}, name?}` (idempotent per endpoint),
      `DELETE /push/subscriptions/{id}`. Endpoint and keys are envelope-encrypted; responses show only `name`, the push
      service host and timestamps. Endpoints must be `https://` and pass the SSRF guard.
- [ ] New channel type `web_push`, config `{users?: [user_id]}` (omitted = every member of the org). At enqueue time
      (ADR-0012) a `web_push` channel fans out into **one job per subscription**, so retries, `410 Gone` (subscription
      deleted, job `failed` with `gone`) and delivery status are per device and visible in `GET /alerts/{id}`.
- [ ] `hm-notify` sends Web Push itself: RFC 8291 (`aes128gcm` payload encryption), RFC 8292 (VAPID ES256 JWT), TTL and
      `Urgency` by severity, over the existing guarded `reqwest` client. Unit test against the RFC 8291 test vector.
- [ ] Service worker shows the notification (title, text, severity tag, `renotify`) and opens `/alerts/{id}` on click;
      the account page has "Enable push on this device" / "Send test push" / remove device. iOS: the UI explains that
      push needs "Add to Home Screen" (iOS 16.4+) and HTTPS.
- [ ] `POST /channels/{id}/test` works for `web_push` (sends to every targeted device, reports per-device errors briefly).
- [ ] Real check (owner): an alert from a **test** project arrives as a push on the owner's phone, through the README
      install path (throwaway Compose project name and ports, HTTPS reachable from the phone).
- [ ] Real check (owner): a fresh install following only the README works end to end, including the M5 check that was
      deferred (bootstrap → login in the UI → connect project → channel test → alert).

### Both

- [ ] Docs: README (UI-first guide, HTTPS for push and phones, reverse proxy), `docs/design/api.md`, `security.md`
      (CSP, push secrets, VAPID), `configuration.md`, `data-model.md`, `alerting.md` (web_push fan-out),
      `ARCHITECTURE.md`, `CLAUDE.md` (commands, status), new `docs/design/web.md`, roadmap, CHANGELOG, ADRs below.

## Context

Read on 2026-11-13: `CLAUDE.md`, `ARCHITECTURE.md`, `docs/roadmap.md`, `docs/product.md`, `docs/design/api.md`,
`docs/design/security.md`, `docs/design/configuration.md`, ADR-0002, -0005, -0007, -0009, -0012, -0013, -0014; the M5b plan.

Verified in the code:

- `crates/server/src/http.rs` builds the router only with `utoipa_axum::routes!`; `tests/openapi.rs` rejects plain
  `.route(`. Static files go in as a `fallback_service` in `http.rs` (no route, not part of the OpenAPI document).
- `crates/notify/src/config.rs`: `ChannelConfig` enum with five types, `KINDS`; `notification_channels.type` has a
  `CHECK` constraint (migration 0006) → a migration widens it. `notification_jobs` is `UNIQUE (alert_event_id,
  channel_id)` → per-subscription jobs need a nullable `push_subscription_id` and a new unique index
  (`NULLS NOT DISTINCT`, Postgres 15+; compose uses `postgres:18-alpine`).
- `crates/server/src/keys.rs` handles `hetzner_projects` and `notification_channels` → two more tables to rotate.
- Notification links are built as `<base>/api/v1/alerts/{id}` (`hm_notify::payload`, tests in `render.rs`,
  `tests/notifications.rs`).
- CI has a "No OpenSSL in the dependency tree" step. The `web-push` crate (0.11) pulls `ece` (OpenSSL backend) and
  `isahc`/`hyper-tls` → not usable; implementing RFC 8291/8292 with RustCrypto is about 200 lines.
- Local tooling: Node 24.15, npm 11.12.

Design system: the owner's design system artifact (link in the owner's notes, deliberately not in this public repo):
React components (Button, Badge, Card, Dialog, Tabs, NavBar, form controls, Toast, Tooltip), tokens (`tokens.css`),
Geist/Geist Mono, Lucide icons at 1.5 px. It was made for a consultancy website, so it has no tables, status badges or
charts.

## Decisions (each becomes an ADR)

1. **ADR-0015 — Web UI embedded in the binary.** `web/` is built by Vite to `web/dist`, embedded with `rust-embed`
   (feature `web-ui`) and served by Axum from `/`. Same origin as the API: the `SameSite=Lax` session cookie and CSRF
   header work without CORS; still one image, one process (ADR-0002); the UI version always matches the API version.
   Rejected: a separate nginx container (second artefact, CORS or proxy config for every self-hoster); serving from a
   directory at runtime (the image would need the files and the binary could drift from them).
2. **ADR-0016 — Frontend tooling.** TypeScript (strict), React 19, Vite, React Router, TanStack Query (server state,
   retries, cache invalidation after mutations), `openapi-typescript` + `openapi-fetch` (types generated from our own
   document, ~6 KB runtime), Lucide React, Geist via npm (bundled, no Google Fonts), Vitest + Testing Library, ESLint +
   Prettier, npm with a committed lockfile. Hand-written service worker instead of Workbox (push handling plus a small
   app-shell cache does not need it). Design-system components are re-implemented in `web/src/ui/` from its tokens and
   rules, without the brand logo. Each npm dependency is justified in the PR.
3. **ADR-0017 — Web Push.** Own RFC 8291/8292 implementation in `hm-notify` (crates `p256`, `hkdf`; `aes-gcm`, `sha2`,
   `base64` already present); VAPID keys generated once per installation and sealed in the database like other secrets;
   subscriptions per user and device, sealed; `web_push` is a channel type whose jobs fan out per subscription at
   enqueue time; `410`/`404` from the push service deletes the subscription. Rejected: the `web-push` crate (OpenSSL),
   VAPID keys in env vars (one more secret to generate and back up by hand, and a plaintext secret in `.env`), one job
   per channel (a single dead phone would make the whole job retry and re-send to the others).

## Approach

### M6a

1. [ ] ADR-0015, ADR-0016 (Proposed); `docs/design/web.md` (structure, routing, auth flow, CSP, build).
2. [ ] Scaffold `web/` (Vite, TS, ESLint, Prettier, Vitest), design tokens and base components from the design system,
       app shell (nav, layout at 375 px and desktop), generated API client + `api.ts` wrapper (CSRF, 401).
3. [ ] Server: `web-ui` feature with `rust-embed`, fallback service, security and cache headers, placeholder page;
       integration tests; notification links → `/alerts/{id}`.
4. [ ] Login, logout, `/me` bootstrap, password change.
5. [ ] Alerts list + detail + ack; projects list/connect/detail/inventory/sync.
6. [ ] Rules, channels (incl. test), silences, API keys.
7. [ ] Manifest, icons, service worker (app shell only).
8. [ ] Dockerfile Node stage, CI `web` job, generated-client freshness check.
9. [ ] README rewritten UI-first; other docs; agent walk-through of the README on a throwaway stack
       (`docker compose -p hm-m6-check`, other ports — never the owner's `hetzner-monitor` project).

### M6b

1. [ ] ADR-0017 (Proposed).
2. [ ] Migration: `vapid_keys`, `push_subscriptions` (org_id, user_id, sealed endpoint/keys, name, host, timestamps),
       channel type `web_push`, `notification_jobs.push_subscription_id` + unique index; `keys status|rotate` cover the
       new tables.
3. [ ] `hm-notify`: RFC 8291 encryption, VAPID JWT, sender; RFC test vector; payload rendering (≤ 3 KB).
4. [ ] Server: VAPID key init at startup, push-subscription endpoints (OpenAPI), `web_push` channel validation, fan-out
       at enqueue, notifier handling of `410`/`404`, channel test.
5. [ ] UI: enable/disable push on this device, device list, test push, iOS hint; service worker `push` and
       `notificationclick`.
6. [ ] Docs; real checks by the owner (push on the phone; fresh README install end to end); roadmap ticks.

## Out of scope

- Tauri apps and native push (APNs/FCM directly) — later per ADR-0009.
- User management in the UI (invite, roles, several orgs per user); TOTP/OIDC.
- Metric graphs (`GET /resources/{id}/metrics` stays proposed), dashboards.
- Offline use of data; i18n (UI copy is English in M6; German later).
- Release v0.2.0 itself: the agent prepares it, the owner pushes the tag.

## Risks / open questions

- **HTTPS for the phone.** Web Push and the service worker need a secure context; iOS needs the PWA added to the home
  screen. The owner needs an HTTPS URL reachable from the phone for the real check (own domain behind Caddy, Tailscale
  `serve`, or a Cloudflare tunnel). **Owner: which one?**
- **Design system in a public AGPL repo.** The plan copies token values and re-implements the components, without the
  logo, inside `web/`. **Owner: is that OK, or should the repo get only a neutral theme derived from it?**
- UI language English (matches code/docs) — **owner: OK for M6?**
- React 19 while the design system was written for React 18: its components are plain function components; low risk.
- `rust-embed` reads files from disk in debug builds; `cargo test` without `web/dist` must still pass (placeholder).
- Push services rate-limit and drop messages without an urgency/TTL; payload size limit ~4 KB.
- Apple's push endpoints (`web.push.apple.com`) require a VAPID `sub`; enforced by requiring `HM_VAPID_SUBJECT` or an
  `https://` `HM_BASE_URL` before push can be enabled.

## Progress log

- 2026-11-13 — Plan drafted after reading the docs, ADRs, the notify/server code and the owner's design system.
- 2026-11-13 — Owner approved the plan without changes. Open questions answered by the plan's defaults until the owner
  says otherwise: design system re-implemented in `web/` without the logo or a link to it; UI copy in English; the
  HTTPS setup for the phone is decided before the M6b real check. M6a started on branch `feat/m6a-web-ui`.
