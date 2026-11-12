# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), versions follow [Semantic Versioning](https://semver.org/)
as described in [ADR-0014](docs/adr/0014-release-and-versioning.md): before 1.0, a minor release may change the API
or configuration — such changes are listed under **Changed** with what to do.

Upgrade by setting `HM_VERSION` in `.env` and running `docker compose pull && docker compose up -d`
([README](README.md#upgrading)). Database migrations run on startup and are forward-only.

## [Unreleased]

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
