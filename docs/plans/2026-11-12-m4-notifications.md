# Plan: M4 notification channels, notifier queue, silences, ack

- Status: In progress
- Milestone: M4
- Owner: Claude (agent), reviewed by Max Nölp
- Started: 2026-11-12
- PR: <link once opened>

## Goal

Alerts reach people. Firing, resolved and re-notified alerts are delivered to email, Slack, Discord, a signed
webhook or ntfy through a durable Postgres queue with retry; users can acknowledge alerts and silence a scope for a
time window. Every channel can be verified with `POST /channels/{id}/test`.

## Acceptance criteria

- [x] `POST /channels` creates a channel of type `email | slack | discord | webhook | ntfy`; its config (URLs,
      recipients, tokens, signing secret) is envelope-encrypted (AAD = channel id) and never returned — the API shows
      only a hint (URL host, recipient count). `GET /channels`, `PATCH /channels/{id}` (name, enabled, min_severity,
      config), `DELETE /channels/{id}`.
- [x] `POST /channels/{id}/test` sends a test notification synchronously and returns `{ "ok": true }` or
      `{ "ok": false, "error": "<redacted reason>" }`; errors never contain the URL, token or secret.
- [x] Routing: an alert event goes to the rule's pinned channels if it has any (`rule_channels`, set via
      `PATCH /rules/{id}` `channel_ids`), otherwise to every enabled channel of the org whose `min_severity` ≤ the
      alert's severity. Pinned channels also respect `enabled`.
- [x] Events `fired`, `resolved` and `renotified` enqueue one `notification_jobs` row per routed channel **in the
      cycle's transaction** (invariant 9); `pending` never notifies. A `resolved` is only sent where its `fired` was routed.
- [x] Re-notify: a firing, unacknowledged alert of severity `warning`/`critical` re-notifies every `renotify_s`
      (rule column, default 4 h); `info` alerts never re-notify.
- [x] `POST /alerts/{id}/ack` sets `acked_at`/`acked_by`, records an `acked` event and stops re-notify; `resolved` is
      still sent. Acking a resolved alert → `409`; acking twice is idempotent.
- [x] Silences: `GET /silences`, `POST /silences` (`project_id` | `resource_id` | `selector`, optional `rule_id`,
      `starts_at`, `ends_at`, `reason`), `DELETE /silences/{id}` (ends it now). Evaluation continues; a notification
      that a silence covers is not queued and recorded as a `suppressed` event (with the silence id).
- [x] Notifier worker (modes `all` and `worker`): sends due jobs with `FOR UPDATE SKIP LOCKED`, exponential backoff
      (30 s · 2ⁿ, capped at 1 h, honours `Retry-After` on 429), gives up after 8 attempts (`failed`, `last_error`
      redacted). A restart loses nothing; delivery is at-least-once. Finished jobs are deleted after 90 days.
- [x] Webhook: JSON payload with `"version": 1` (documented schema), header `X-HM-Signature: sha256=<hex HMAC-SHA256
      of the raw body>` with the channel's signing secret; `X-HM-Event` and `X-HM-Delivery` (job id) headers.
- [x] Email via SMTP (`HM_SMTP_HOST`, `_PORT`, `_USER`, `_PASSWORD`, `_FROM`, `_TLS` = `starttls|tls|none`); without
      `HM_SMTP_HOST` creating an email channel → `422`. Links in messages use `HM_BASE_URL` when set.
- [x] SSRF guard: with `HM_ALLOW_PRIVATE_TARGETS=false`, webhook/ntfy/Slack/Discord targets resolving to private,
      loopback, link-local, CGNAT or metadata addresses are refused (checked after DNS resolution, redirects disabled).
      Default `true` (self-hosted).
- [x] Tests: unit tests for rendering per channel, signature, routing, silence matching, re-notify timing, backoff;
      integration tests (wiremock for HTTP channels, lettre stub transport for email, synthetic clock): fire → job →
      delivered; resolved follows; retry after 500 and 429; silence suppresses; ack stops re-notify; restart-safety
      (job left queued is sent by a new worker); tenant isolation of channels and silences.
- [ ] **Real check** (`scripts/smoke-m4.sh`): the owner creates each channel type they want to verify from env vars
      (never typed into the chat), runs `POST /channels/{id}/test` for each, confirms arrival; channels are deleted again.
      No Hetzner write access; no production server is touched.
- [x] fmt, clippy `--locked -D warnings`, tests green; docs updated (alerting, data-model, api, configuration,
      security, ARCHITECTURE status, CLAUDE.md status + commands, roadmap); ADR-0012.

## Context

Read and verified on 2026-11-12:

- [roadmap](../roadmap.md) M4 row; [ARCHITECTURE](../../ARCHITECTURE.md) invariants 3 (no plaintext secrets), 5
  (tenant scope), 9 (durable notifications); `hm-notify` is an empty crate that must not depend on internal crates.
- [alerting](../design/alerting.md): notify on `firing`/`resolved` only, re-notify every `renotify_s` unless acked,
  silences suppress and record, channel table, versioned webhook schema, `notification_jobs` with backoff. Proposal
  "no re-notify for `info`" — adopted here.
- [data-model](../design/data-model.md): `notification_channels`, `rule_channels`, `notification_jobs`, `silences`
  already sketched; `alerts` already has `last_notified_at`, `acked_by`, `acked_at`; `alert_events.type` already allows
  `acked | renotified | suppressed`; `rules.renotify_s` exists (≥ 300, default 14400) — migration 0003.
- [security](../design/security.md): envelope encryption for channel configs (`hm_core::crypto::seal/open`, AAD = row
  id), SSRF rule with `HM_ALLOW_PRIVATE_TARGETS`, redaction. [configuration](../design/configuration.md): `HM_SMTP_*`,
  `HM_BASE_URL`, `HM_ALLOW_PRIVATE_TARGETS` planned.
- Code: alert transitions in `crates/server/src/engine.rs` (`Transition::{OpenFiring, Fire, StayFiring, Resolve}`) and
  `repo/alerts.rs` (`fire`, `resolve`, `event`) run in the cycle's transaction — the enqueue point. `reqwest` 0.13
  (rustls, no provider) is already a workspace dependency; `sha2` 0.11 too.
- External facts (2026-11-12): ntfy publish = `POST <server>/<topic>` with body text and headers `X-Title`,
  `X-Priority` (1–5), `X-Tags`, `X-Click`, auth `Authorization: Bearer <token>` (docs.ntfy.sh/publish). Slack incoming
  webhook = `POST` JSON `{ "text" }`, Discord webhook = `POST` JSON `{ "content" }` (≤ 2000 chars) or `embeds`; both
  rate-limit with `429` + `Retry-After`. Crates: `lettre` 0.11.23 (features `tokio1-rustls`, `ring`, `webpki-roots`,
  `smtp-transport`, `builder`), `hmac` 0.13.0 (needs `digest` 0.11 = our `sha2` 0.11).

## Approach

1. [x] **ADR-0012** — notification delivery: at-least-once queue in Postgres, jobs created in the alert transaction,
       worker sends while holding the row lock (`SKIP LOCKED`, short send timeout), routing and silence evaluation at
       enqueue time, no re-notify for `info`.
2. [x] **Migration `0006_notifications.sql`** — `notification_channels` (encrypted config, `hint`, `min_severity`,
       `enabled`), `rule_channels`, `notification_jobs` (`alert_event_id`, `channel_id`, `status queued|sent|failed`,
       `attempts`, `next_attempt_at`, `last_error`, `sent_at`; index on due jobs), `silences` (+ optional `rule_id`).
       All tenant-scoped via `org_id` or parent FK.
3. [x] **`hm-notify`** — `Payload` (version 1: event, alert, rule, project, resource, link), `ChannelConfig` enum
       (secret-holding, no `Debug` content), renderers (`slack`, `discord`, `ntfy`, `webhook`, `email` text), a `Sender`
       with `send(config, payload) -> Result<(), SendError>` where `SendError` is `Retry { after }` / `Permanent` with
       redacted reasons; HMAC signature; SSRF check (resolve, reject ranges). New crates: `lettre`, `hmac` (justified in PR).
4. [x] **Config** — `HM_SMTP_*`, `HM_BASE_URL`, `HM_ALLOW_PRIVATE_TARGETS`; SMTP password via redacted secret type.
5. [x] **Enqueue in the engine** — on `fired`, `resolved`, re-notify: match silences (core: scope + selector + window),
       route channels, insert jobs or a `suppressed` event; update `last_notified_at`. Pure routing/silence/re-notify
       logic in `hm-core` (unit-testable).
6. [x] **Notifier worker** — Tokio task next to the scheduler: claim due jobs, decrypt config in memory, send, update
       status/backoff; retention delete; graceful shutdown.
7. [x] **API** — channels CRUD + test, `PATCH /rules/{id}` `channel_ids`, `POST /alerts/{id}/ack`, silences;
       `GET /alerts/{id}` events show suppression; scope checks (`write` for changes).
8. [x] **Tests** — unit + integration as in the criteria; existing scheduler tests stay green.
9. [x] **Docs + `scripts/smoke-m4.sh`** (macOS Python 3.9, secrets from env / `deploy/.env` only, channels deleted on exit).
10. [ ] **Verify** — criteria, PR, owner's real channel check, record the result.

## Dependencies (new crates)

- `lettre` 0.11 — SMTP with rustls/ring and Tokio; the standard Rust mail crate, already named in the design.
- `hmac` 0.13 — webhook signature; RustCrypto, same family as `sha2`/`aes-gcm` we already use.

## Out of scope

- Web Push and `push_subscriptions` (M6, with the PWA); native push (Tauri).
- Per-user notification preferences, quiet hours, digests/grouping of many alerts into one message.
- Un-acknowledge, editing silences (create + end only), silence templates.
- Login/sessions (M5): ack is attributed to the API key's user.
- Retention of `alert_events` (only finished jobs are cleaned up here).

## Risks / open questions

- **At-least-once:** a crash between "sent" and the commit re-sends once. Accepted; the webhook carries
  `X-HM-Delivery` for receivers to dedupe.
- **Holding a row lock during the HTTP call** keeps a transaction open up to the send timeout (10 s). Fine for the
  self-hosted scale; revisit with a lease column if jobs pile up.
- **Alert storms** (e.g. many servers at once): one message per alert per channel; Slack/Discord rate limits are
  handled by backoff, not by grouping. Grouping is a later improvement.
- **Hygiene `info` alerts** fire at once after connect; with a default "all channels" route they would all be sent once.
  Proposal: channels default to `min_severity = warning`, so `info` only reaches channels that opt in.
- **SMTP real check** needs the owner's SMTP credentials; can be skipped if only HTTP channels are verified.

## Progress log

- 2026-11-12 — Plan drafted from the alerting/data-model/security docs, the engine code and the ntfy/lettre/hmac facts.
- 2026-11-12 — Approved and implemented. Deviations and details: (1) the queue is filled from `repo::alerts` (every
  `fired`/`resolved` event, also from `close_gone` and disabling a rule) and re-notify from the engine's still-firing
  branch (not during the flapping guard's good cycles); pre-M4 firing alerts use `fired_at` as their last notification.
  (2) `last_notified_at` is also set when a notification is suppressed, so a silence ending does not trigger an immediate
  re-notify. (3) Pinned channels ignore `min_severity` (explicit choice) but respect `enabled`. (4) `Retry-After` replaces
  the backoff delay (1 s … 1 h) instead of only extending it. (5) Channel names are unique per org (case-insensitive
  index, `409 name_taken`); new channels default to `min_severity = warning`. (6) The SSRF guard is a reqwest DNS
  resolver plus an IP-literal check; redirects and proxies are off. (7) Links point to `HM_BASE_URL/api/v1/alerts/{id}`
  until the PWA exists. (8) `hm-notify` has its own `Severity` and string ids, so it still depends on nothing internal.
  (9) `deploy/docker-compose.yml` passes the new `HM_*` variables through; `deploy/.env.example` is not updated
  (the agent may not read it) — the variables are documented in `docs/design/configuration.md`.
  Tests: 14 `hm-notify` unit tests (rendering, signature, SSRF ranges, retry classification, stub email), core delivery
  timing, config parsing; 10 integration scenarios in `tests/notifications.rs`; all existing suites green. `smoke-m4.sh`
  verified end to end against a throwaway local instance with a local webhook receiver.
