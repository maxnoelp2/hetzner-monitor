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
| **M6** | React PWA incl. Web Push | An alert arrives as a push on a phone |
| later | Tauri 2 mobile → Tauri 2 desktop → SaaS (multi-worker, KMS, billing) | |

## Pre-M3 verifications

- [ ] CPU metric is per vCPU (100 = one full core) — verify on a real multi-core server.
- [ ] Traffic billing period — not exposed by the API; confirm the calendar-month assumption.

## M2 — in progress ([plan](plans/2026-11-11-m2-scheduler-rules-alerts.md))

- [x] Scheduler: fast (60 s) + inventory (15 min) tiers, budget maths, leases, 429/401/5xx handling
- [x] Rule engine, alert state machine, default rules, label selectors
- [x] Checks `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota`; system alert `hetzner_api`
- [x] Rules and alerts API
- [ ] Owner's power-off test on a dedicated test server (`scripts/smoke-m2.sh`)

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
