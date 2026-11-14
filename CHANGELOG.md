# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow [Semantic Versioning](https://semver.org/)
as described in [ADR-0014](docs/adr/0014-release-and-versioning.md): before 1.0, a minor release may change the API
or configuration — such changes are listed under **Changed** with what to do.

Upgrade by setting `HM_VERSION` in `.env` and running `docker compose pull && docker compose up -d`
([README](README.md#upgrading)). Database migrations run on startup and are forward-only.

## [Unreleased]

### Added

- **Web UI:** a React PWA served by the binary on the same origin as the API (`http://127.0.0.1:8080/`). Log in with
  e-mail and password; alerts (filters, detail with events and deliveries, ack), projects (connect, schedule, budget,
  cost estimate, inventory, sync), rules (edit, enable/disable, custom rules), channels (create, edit, test, delete),
  silences, API keys and password change. Installable as an app (manifest and a service worker that caches only the app
  shell).
- **Push notifications (Web Push):** new channel type `web_push` that sends an alert to every subscribed phone or
  computer of the org's members (or of chosen users), one delivery per device with its own retries and status. Turn it
  on per device under **Settings → Push notifications** (iPhone/iPad: add the app to the home screen first). Needs HTTPS
  and `HM_VAPID_SUBJECT` or an `https://` `HM_BASE_URL`; the signing key is generated and stored encrypted
  automatically and covered by `keys rotate`. New endpoints `GET /push/config`, `GET`/`POST /push/subscriptions`,
  `DELETE /push/subscriptions/{id}`, `POST /push/subscriptions/{id}/test`; alert deliveries show the `device`.
- README: HTTPS with Tailscale Serve (private, no domain) or a reverse proxy; push setup on phones.
- Strict security headers on UI responses (Content Security Policy, `nosniff`, `Referrer-Policy: same-origin`); the UI
  loads nothing from third-party hosts.
- Cargo feature `web-ui` embeds the built UI; without it (plain `cargo build`) non-API paths show a placeholder page.
  The Docker image and releases enable it.

### Changed

- Notification links now open the alert in the web UI: `<HM_BASE_URL>/alerts/{id}` instead of
  `<HM_BASE_URL>/api/v1/alerts/{id}`. Nothing to do; scripts that followed the old link to fetch JSON should call
  `GET /api/v1/alerts/{id}` with an API key instead.
- `GET`/`HEAD` requests to paths outside `/api` that are not an API route (e.g. `/`, `/alerts`) now return the web UI
  (or, in a binary built without it, a placeholder page) instead of an empty 404. Unknown paths under `/api` now return
  the usual JSON error `404 not_found` instead of an empty body.
- The self-hosting guide in the README now uses the web UI; the API commands remain as an alternative.

## [0.1.0] - 2026-11-12

First release: a self-hostable server with the HTTP API. The web app (PWA) follows in a later release.

### Added

- **Projects:** connect Hetzner Cloud projects with a read-only API token, stored envelope-encrypted (AES-256-GCM);
  inventory of servers, load balancers, volumes, certificates, floating and primary IPs; on-demand sync.
- **Scheduler:** fast (60 s), inventory (15 min) and adaptive metrics tiers within a rate-limit budget
  (default 50 % of the project's Hetzner limit); a Hetzner API outage raises one `hetzner_api` alert instead of
  "server down" alerts.
- **Checks:** `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota` (with a calendar-month
  forecast), `cpu`, `network`, `disk_io`, `certificate_expiry`, `deprecation`, `hygiene_backups`,
  `hygiene_protection`, `hygiene_orphans`, `cost_change` (daily cost estimate from `/pricing`).
- **Rules and alerts:** default rules per project, custom rules with label selectors, durations and severities;
  alert lifecycle `pending → firing → resolved`, acknowledgements, re-notify.
- **Notifications:** email (SMTP), Slack, Discord, signed webhooks and ntfy from a durable Postgres queue with retries;
  routing by severity or pinned channels; silences; `POST /channels/{id}/test`; optional SSRF guard
  (`HM_ALLOW_PRIVATE_TARGETS=false`).
- **Auth:** API keys with `read`/`write`/`admin` scopes; password login (Argon2id) with an `HttpOnly` session cookie
  and CSRF token; login throttling; `bootstrap` prints the first admin key and password; `users reset-password`.
- **Key rotation:** versioned master keys (`HM_MASTER_KEYS`), `keys status` and `keys rotate`.
- **API description:** OpenAPI 3.1 at `GET /api/v1/openapi.json` and `hetzner-monitor openapi`.
- **Distribution:** multi-arch image `ghcr.io/maxnoelp2/hetzner-monitor` (linux/amd64, linux/arm64) and a Docker
  Compose file with Postgres; self-hosting guide in the README.

### Known limitations

- No web UI yet; use the API (curl, the OpenAPI document) until the PWA lands.
- The Hetzner Cloud API has no RAM or disk-usage metrics; those need an agent (out of scope for now).
- The documented setup is one process (`HM_MODE=all`); separate `api` and `worker` processes are not documented yet.

[Unreleased]: https://github.com/maxnoelp2/hetzner-monitor/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/maxnoelp2/hetzner-monitor/releases/tag/v0.1.0
