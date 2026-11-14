# Roadmap

Each milestone has an acceptance criterion that can be demonstrated. Tick items as they land;
link the execution plan in `docs/plans/` that delivered them.

| # | Scope | Done when … |
|---|---|---|
| **M0** | Cargo workspace (`hcloud`, `core`, `notify`, `server`), Docker Compose (app + Postgres), first migration, `/healthz`, CI (fmt, clippy, test with Postgres service) | `docker compose up` starts; CI is green |
| **M1** | `hcloud` client; bootstrap org + admin; API keys; connect project (encrypted token); inventory sync | A real test project is connected and its inventory is readable via the API |
| **M2** | Scheduler (3 tiers, budget), rule engine, alert lifecycle; checks `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota` | Power off a server → alert `firing`; power on → `resolved` |
| **M3** | Metric checks (`cpu`, `network`, `disk_io`), Hetzner-specific, hygiene, cost | CPU load via `stress` → alert after the rule duration |
| **M4** | Channels (email, Slack, Discord, webhook, ntfy), notifier queue, silences, ack | Every channel verified via `POST /channels/{id}/test` |
| **M5** | Hardening: key rotation, login + sessions, OpenAPI, docs, release v0.1.0 (image on GHCR) | Self-hostable from the README alone |
| **M6** | React PWA incl. Web Push | An alert arrives as a push on a phone; a fresh install from the README works end to end (includes the M5 check the owner deferred) |
| later | Tauri 2 mobile → Tauri 2 desktop → SaaS (multi-worker, KMS, billing) | |

## M6 — in progress ([plan](plans/2026-11-13-m6-pwa-web-push.md), [ADR-0015](adr/0015-web-ui-embedded-in-binary.md), [ADR-0016](adr/0016-frontend-tooling.md))

- M6a — web UI in the binary, session login:
  - [ ] `web/` React + TypeScript PWA (Vite), embedded into the binary and served from `/` on the API's origin; client-side routing fallback to `index.html`, `/api/*` stays a JSON 404
  - [ ] Strict CSP, `nosniff`, `Referrer-Policy: same-origin`; `immutable` hashed assets, `no-cache` for the rest; no third-party requests
  - [ ] Login, session cookie, CSRF token in memory (re-read from `GET /me`), 401 returns to login, logout, password change
  - [ ] Screens: alerts (filters, detail, ack), projects (connect, detail, inventory, sync), rules, channels (incl. test), silences, API keys, account
  - [ ] API client generated from the OpenAPI document (`openapi-typescript`); CI fails on a stale generated file
  - [ ] Owner's design system re-implemented in `web/` (dark-first, Geist, Lucide) plus tables, badges, empty and error states; usable at 375 px
  - [ ] Installable PWA: `manifest.webmanifest`, service worker caching the app shell only
  - [ ] Notification links point to `<HM_BASE_URL>/alerts/{id}`
  - [ ] `cargo build` without Node still works (feature `web-ui`, placeholder page otherwise); Docker image and release enable it
  - [ ] Tests: Rust integration tests for static serving and headers; Vitest + Testing Library for the API wrapper, login and alert ack
  - [ ] CI `web` job (`npm ci`, typecheck, lint, test, build, generated-client check); Docker build with a Node stage
- M6b — Web Push:
  - [ ] VAPID key pair generated on first start, stored envelope-encrypted, covered by `keys status` / `keys rotate`; `HM_VAPID_SUBJECT`
  - [ ] Push-subscription API per user and device (endpoint and keys sealed, SSRF-guarded `https://` endpoints)
  - [ ] Channel type `web_push`, one notification job per subscription (per-device retries, `410 Gone`, delivery status)
  - [ ] RFC 8291 / RFC 8292 sender in `hm-notify` with the RFC test vector
  - [ ] Service worker shows notifications and opens `/alerts/{id}`; enable/test/remove push in the UI; iOS hint
  - [ ] `POST /channels/{id}/test` for `web_push`
  - [ ] Real check (owner): an alert from a test project arrives as a push on the owner's phone
  - [ ] Real check (owner): a fresh install following only the README works end to end (includes the deferred M5 check)

## M5 — done (v0.1.0 released 2026-11-13)

- M5a ([plan](plans/2026-11-12-m5a-auth-sessions-rotation.md), [ADR-0013](adr/0013-sessions-csrf-and-key-rotation.md)):
  - [x] Password login (Argon2id), server-side sessions (`hm_session` cookie), derived CSRF token, `/me`, `PUT /me/password`, login throttling
  - [x] Initial password from `bootstrap`, `users reset-password`
  - [x] Master-key rotation: `HM_MASTER_KEYS`, `keys status` / `keys rotate`, startup check of stored key versions
  - [x] Real check on the local stack (`scripts/smoke-m5a.sh`): login, `/me`, CSRF refusal/acceptance, logout (2026-11-12)
- M5b ([plan](plans/2026-11-12-m5b-openapi-release.md), [ADR-0014](adr/0014-release-and-versioning.md)):
  - [x] OpenAPI 3.1 generated with `utoipa` at `/api/v1/openapi.json` and `hetzner-monitor openapi`; tests keep it complete; Redocly lint in CI
  - [x] README self-hosting guide; Compose on the GHCR image (`HM_VERSION`), build override for development
  - [x] Release workflow (tag `v*` → native amd64/arm64 builds → GHCR manifest → GitHub release), `CHANGELOG.md`, version 0.1.0
  - [ ] Real check: fresh install following only the README (owner, test project) — deferred by owner decision (2026-11-13) until the web UI is complete; README steps 2–5 checked by the agent
  - [x] Release v0.1.0 tagged (2026-11-13), multi-arch image public on GHCR (anonymous pull verified)

## M4 — done ([plan](plans/2026-11-12-m4-notifications.md))

- [x] Channels email (SMTP), Slack, Discord, signed webhook, ntfy; configs sealed, write-only; `POST /channels/{id}/test`
- [x] Durable queue (`notification_jobs`, enqueued in the alert transaction) and notifier with backoff ([ADR-0012](adr/0012-notification-delivery.md))
- [x] Routing by `min_severity` or pinned rule channels; re-notify (not for `info`); ack; silences
- [x] SSRF guard (`HM_ALLOW_PRIVATE_TARGETS=false`)
- [x] Real check (`scripts/smoke-m4.sh`): Discord verified via `POST /channels/{id}/test`, message arrived (2026-11-12); the other types via integration tests

## Pre-M3 verifications

- [x] CPU metric is per vCPU (100 = one full core) — stated explicitly in the spec (2026-11-11); real data on a 3-vCPU server normalised as expected in the M3a smoke test (2026-11-12).
- [x] Traffic billing period — not exposed by the API. Billing FAQ (2026-11-11): only outgoing traffic, invoices per calendar month; the counter reset date is **not** documented → the M3b forecast is a labelled calendar-month estimate (decided 2026-11-12).

## M3 — done

- M3a ([plan](plans/2026-11-11-m3a-metrics-tier.md)):
  - [x] Metrics tier (adaptive interval, only servers in scope of metric rules), series evaluation without local storage
  - [x] Checks `cpu` (default rule, normalised by vCPUs), `network` and `disk_io` (templates)
  - [x] Custom rules: `POST /rules`, `DELETE /rules/{id}`
  - [x] Read-only real check (`scripts/smoke-m3a.sh`): a temporary low-threshold `cpu` rule fired from real Hetzner data (2026-11-12)
- M3b ([plan](plans/2026-11-12-m3b-inventory-checks.md)):
  - [x] Inventory checks on inventory cycles over all six kinds: `certificate_expiry`, `deprecation`, `hygiene_backups`, `hygiene_protection`, `hygiene_orphans`
  - [x] `cost_change` with the cost estimate from `/pricing` × inventory (shown on `GET /projects/{id}`)
  - [x] `traffic_quota` forecast (calendar-month estimate) from daily traffic samples; derived daily snapshots per ADR-0011
  - [x] Read-only real check (`scripts/smoke-m3b.sh`): deprecation and hygiene findings on the real project, cost estimate with every resource priced (2026-11-12)

## M2 — done ([plan](plans/2026-11-11-m2-scheduler-rules-alerts.md))

- [x] Scheduler: fast (60 s) + inventory (15 min) tiers, budget maths, leases, 429/401/5xx handling
- [x] Rule engine, alert state machine, default rules, label selectors
- [x] Checks `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota`; system alert `hetzner_api`
- [x] Rules and alerts API
- [x] Power off → `firing`, power on → `resolved`: verified by integration test against a mocked Hetzner API.
      The real power-off test was **not performed** (owner decision: only production servers available); optional later with a test project and `scripts/smoke-m2.sh`.

## M1 — done ([plan](plans/2026-11-06-m1-connect-inventory.md))

- [x] `hm-hcloud` read-only client (pagination, errors, rate-limit headers)
- [x] `bootstrap` CLI, API keys with scopes
- [x] Connect project with envelope-encrypted token, inventory sync, resources API
- [x] Real-project smoke test by the owner (`scripts/smoke-m1.sh`)

## M0 checklist — done ([plan](plans/2026-11-06-m0-skeleton.md))

- [x] Cargo workspace with four crates, edition 2024
- [x] `deploy/docker-compose.yml` (app + postgres)
- [x] `migrations/0001_init.sql` (organizations, users, memberships)
- [x] `GET /healthz`, `GET /readyz`
- [x] Multi-stage Dockerfile (rustls, small runtime image)
- [x] GitHub Actions: fmt, clippy `-D warnings`, test against Postgres service
- [x] Update "Commands" in `CLAUDE.md`
