# Design: HTTP API

Status: endpoints marked ✅ are **implemented** (M1–M4); the rest **proposed** · Crate: `crates/server`

REST under `/api/v1`, JSON. The OpenAPI document is **generated from code** (`utoipa`) and served at
`/api/v1/openapi.json`; the PWA's TypeScript client is generated from it.

```
POST   /auth/login · POST /auth/logout · GET /me
✅ GET  /projects · ✅ POST /projects {name, token} → 201 {project, sync} · ✅ GET/DELETE /projects/{id} · PATCH /projects/{id}
✅ POST /projects/{id}/sync           → re-sync inventory now (any status; 409 while a cycle runs) → {sync, project}
✅ GET  /projects/{id}/resources?kind= → inventory (server|load_balancer|volume|certificate|floating_ip|primary_ip)
GET    /resources/{id}/metrics       → proxy to Hetzner metrics (for later graphs; budgeted)
✅ GET /rules?project_id= · ✅ GET /rules/{id} → {rule, channel_ids} · ✅ PATCH /rules/{id} {name?, enabled?, params?, severity?, selector?, channel_ids?}
✅ POST /rules {project_id?, name, check_type, params?, severity, selector?, resource_id?, enabled?} → 201 {rule} · ✅ DELETE /rules/{id} (custom rules only)
✅ GET /alerts?state=open|pending|firing|resolved&project_id=&page=&per_page= → {alerts, meta.pagination} · ✅ GET /alerts/{id} → {alert, events, notifications}
✅ POST /alerts/{id}/ack → {alert} (idempotent; resolved → 409 alert_resolved)
✅ GET /channels · ✅ POST /channels {type, name, config, min_severity?, enabled?} → 201 {channel} · ✅ GET/PATCH/DELETE /channels/{id}
✅ POST /channels/{id}/test → {ok: true} | {ok: false, error, retryable}
✅ GET /silences?active=true|false · ✅ POST /silences {project_id?, resource_id?, rule_id?, selector?, starts_at?, ends_at, reason?} → 201 {silence} · ✅ DELETE /silences/{id} (ends it now)
✅ GET /api-keys · ✅ POST /api-keys {name, scopes} → 201 {api_key, key} (key shown once) · ✅ DELETE /api-keys/{id} (revoke)
GET    /healthz · GET /readyz · GET /metrics (Prometheus, own operational metrics; unauthenticated only if bound to localhost/internal)
```

## Conventions

- Auth: session cookie or `Authorization: Bearer hm_…` ([security](security.md#authentication)).
- Errors: `{ "error": { "code": "snake_case", "message": "…" } }` with proper HTTP status.
- Pagination: `?page=&per_page=` (max 100), response `meta.pagination` — same shape as Hetzner's.
- IDs are UUIDs; Hetzner IDs appear only as `hetzner_id`.
- Secrets are write-only: accepted on create/update, never returned.
- Scopes: `read` for GET, `write` for project changes and sync, `admin` for API keys. `admin ⊃ write ⊃ read`.
- Project responses include the schedule and budget: `failures`, `last_fast_at`, `next_fast_at`, `next_inventory_at`,
  `last_metrics_at`, `next_metrics_at`, `budget_share`, `budget_h`, `fixed_h`, `metrics_interval_s` ([scheduler](scheduler.md)).
- `GET /projects/{id}` also returns `cost_estimate`: `{day, monthly_net, currency, unpriced, at}` (latest daily estimate, net,
  see [checks](checks.md#cost-estimate)) or `null` before the first inventory cycle.
- `PATCH /rules/{id}`: `selector: null` or `""` clears the selector; params are validated per check; unknown fields → 400.
- `POST /rules`: `project_id` must be a project of the org (omit = all projects of the org); `resource_id` needs `project_id`
  and must belong to it; `hetzner_api` cannot be created (one system rule per project). Deleting a rule deletes its alerts;
  default rules → `409 default_rule` (disable them instead).
  Rules are writable with the `write` scope.
- `PATCH /rules/{id}` `channel_ids`: pins the rule to these channels of the org (`[]` unpins; unknown id → 400);
  the response carries the rule and its `channel_ids`.
- **Channels** (`write` scope to change): `type` is `email | slack | discord | webhook | ntfy` with `config`
  `{to: [addresses]}` · `{url}` · `{url}` · `{url, secret (16–256 chars)}` · `{server? (default https://ntfy.sh), topic, token?}`.
  The config is write-only (sealed at rest); responses show `hint` (URL host or recipient count). `min_severity` defaults
  to `warning`. Names are unique per org (`409 name_taken`). Email needs `HM_SMTP_HOST` on the server (`422 email_disabled`).
  `PATCH` replaces `config` completely when given. `POST /channels/{id}/test` sends now, also to disabled channels; errors are
  short and redacted (`HTTP 404`, `timeout`, `target address not allowed`).
- **Silences:** all scope fields optional (none = the whole org); `selector` uses the rule selector syntax and is matched
  against resource labels; at most 365 days; `ends_at` must be in the future. `GET` lists only not-yet-ended silences unless
  `active=false`. `DELETE` sets `ends_at` to now (a future silence is removed).
- `GET /alerts/{id}` `notifications`: one entry per delivery job `{id, alert_event_id, channel_id, channel_name, status
  (queued|sent|failed), attempts, last_error, next_attempt_at, finished_at}`; the alert shows `acked_at`, `acked_by`, `last_notified_at`.
- Implemented error codes: `unauthorized` (401), `forbidden` (403), `not_found` (404, also for other tenants' ids and malformed ids),
  `bad_request` (400), `name_taken` (409), `sync_in_progress` (409), `default_rule` (409), `invalid_token` (422, connect), `hetzner_invalid_token` / `hetzner_unavailable` (502),
  `hetzner_rate_limited` (503), `internal` (500).
