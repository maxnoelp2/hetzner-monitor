# Design: HTTP API

Status: endpoints marked ✅ are **implemented** (M1–M6b); the rest **proposed** · Crate: `crates/server`

REST under `/api/v1`, JSON. The OpenAPI 3.1 document is **generated from code** (`utoipa` 6 + `utoipa-axum` 0.3) and
served at `GET /api/v1/openapi.json` (unauthenticated) and printed by `hetzner-monitor openapi`; the PWA's TypeScript
client is generated from it.

The web UI is served from `/` (M6a, [web](web.md#serving-and-headers)): every `GET`/`HEAD` path that matches no route
and is not under `/api` serves the embedded UI (an existing file, otherwise `index.html` for client-side routing). This
is the router's fallback, not a route, so it is not part of the OpenAPI document. Unknown paths under `/api` (any
method) and non-`GET`/`HEAD` requests outside `/api` get the JSON `404 not_found`; API clients never get HTML.

## OpenAPI

- Every handler has a `#[utoipa::path]` with a unique camelCase `operationId` (`listProjects`, `createChannel`, …), a tag,
  parameters, request body, success response and its specific error responses. Routes are registered **only** with
  `utoipa_axum::routes!` in `crates/server/src/http.rs` and `crates/server/src/api/mod.rs`; `tests/openapi.rs` fails on
  plain Axum routing there and checks that every documented operation reaches a handler, so the document lists exactly
  the router's routes.
- Handlers return typed structs (`Serialize` + `ToSchema`), so response schemas are derived from what is serialised.
  Text columns with a closed set of values (severity, alert state, resource kind, channel type, …) are documented as
  string enums (`crate::openapi::enums`). In response schemas every field is required (`None` is sent as `null`), except
  fields skipped when empty (`Health.reason`, `TestResult.error`/`retryable`/`devices`, `DeviceResult.error`,
  `PushConfig.vapid_public_key`, `PushTestResult.error`/`gone`); request `Option` fields may be omitted.
- Security schemes: `api_key` (HTTP Bearer), `session` (cookie `hm_session`), `csrf` (header `X-CSRF-Token`). The
  requirement per operation is derived from the method like the auth layer does: GET → `api_key` or `session`;
  other methods → `api_key` or `session` + `csrf`; `/auth/login`, `/healthz`, `/readyz`, `/openapi.json` → none.
  Authenticated operations reference the shared `401 Unauthorized` and `403 Forbidden` responses.
- CI lints the document with Redocly CLI (`redocly.yaml`, accepted findings in `.redocly.lint-ignore.yaml`).

```
✅ POST /auth/login {email, password, org_id?} → {user, org, role, auth, csrf_token} + Set-Cookie hm_session · ✅ POST /auth/logout → 204
✅ GET /me → {user: {id, email, name}, org: {id, name}, role, auth: session|api_key, csrf_token (session only)}
✅ PUT /me/password {current_password, new_password} → 204 (ends the user's other sessions)
✅ GET  /projects · ✅ POST /projects {name, token} → 201 {project, sync} · ✅ GET/DELETE /projects/{id} · PATCH /projects/{id}
✅ POST /projects/{id}/sync           → re-sync inventory now (any status; 409 while a cycle runs) → {sync, project}
✅ GET  /projects/{id}/resources?kind= → inventory (server|load_balancer|volume|certificate|floating_ip|primary_ip)
GET    /resources/{id}/metrics       → proxy to Hetzner metrics (for later graphs; budgeted)
✅ GET /rules?project_id= · ✅ GET /rules/{id} → {rule, channel_ids} · ✅ PATCH /rules/{id} {name?, enabled?, params?, severity?, selector?, channel_ids?}
✅ POST /rules {project_id?, name, check_type, params?, severity, selector?, resource_id?, enabled?} → 201 {rule} · ✅ DELETE /rules/{id} (custom rules only)
✅ GET /alerts?state=open|pending|firing|resolved&project_id=&page=&per_page= → {alerts, meta.pagination} · ✅ GET /alerts/{id} → {alert, events, notifications}
✅ POST /alerts/{id}/ack → {alert} (idempotent; resolved → 409 alert_resolved)
✅ GET /channels · ✅ POST /channels {type, name, config, min_severity?, enabled?} → 201 {channel} · ✅ GET/PATCH/DELETE /channels/{id}
✅ POST /channels/{id}/test → {ok: true} | {ok: false, error, retryable} · web_push: {ok, error?, devices: [{id, name, ok, error?}]}
✅ GET /push/config → {enabled, vapid_public_key?}
✅ GET /push/subscriptions → {subscriptions} (the caller's devices) · ✅ POST /push/subscriptions {endpoint, keys: {p256dh, auth}, expirationTime?, name?} → 201 | 200 {subscription}
✅ DELETE /push/subscriptions/{id} → 204 · ✅ POST /push/subscriptions/{id}/test → {ok, error?, gone?}
✅ GET /silences?active=true|false · ✅ POST /silences {project_id?, resource_id?, rule_id?, selector?, starts_at?, ends_at, reason?} → 201 {silence} · ✅ DELETE /silences/{id} (ends it now)
✅ GET /api-keys · ✅ POST /api-keys {name, scopes} → 201 {api_key, key} (key shown once) · ✅ DELETE /api-keys/{id} (revoke)
✅ GET /api/v1/openapi.json → OpenAPI 3.1 document (no auth)
✅ GET /healthz · ✅ GET /readyz → {status, reason?} · GET /metrics (Prometheus, own operational metrics; unauthenticated only if bound to localhost/internal)
```

## Conventions

- Auth: session cookie or `Authorization: Bearer hm_…` ([security](security.md#authentication)). Cookie requests other than
  `GET`/`HEAD`/`OPTIONS` need `X-CSRF-Token` (from login or `GET /me`). A session has the scope of its role
  (`owner`/`admin` → `admin`, `member` → `write`).
- Login: wrong e-mail, wrong password, disabled user or no password → the same 401 `invalid_credentials`; unknown fields → 400;
  `org_id` selects one of the user's orgs (default: the oldest membership). Throttled → 429 `too_many_attempts` + `Retry-After`.
- `PUT /me/password`: new password 12–1024 characters (400 otherwise); wrong `current_password` → 403 `wrong_password`.
  A user without a password (bootstrapped before M5a) sets the first one with an `admin` API key, without `current_password`.
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
- **Channels** (`write` scope to change): `type` is `email | slack | discord | webhook | ntfy | web_push` with `config`
  `{to: [addresses]}` · `{url}` · `{url}` · `{url, secret (16–256 chars)}` · `{server? (default https://ntfy.sh), topic, token?}`
  · `{users?: [user_id]}` (1–100 members of the org; omitted = every member).
  The config is write-only (sealed at rest); responses show `hint` (URL host, recipient count, `all members` / `N users`). `min_severity` defaults
  to `warning`. Names are unique per org (`409 name_taken`). Email needs `HM_SMTP_HOST` on the server (`422 email_disabled`).
  `PATCH` replaces `config` completely when given. `POST /channels/{id}/test` sends now, also to disabled channels; errors are
  short and redacted (`HTTP 404`, `timeout`, `target address not allowed`). A `web_push` channel needs Web Push on the
  server (`422 push_disabled`, see `HM_VAPID_SUBJECT`); its test sends to every targeted device and reports each in
  `devices`; a device whose push service answers `404`/`410` is removed ([ADR-0017](../adr/0017-web-push.md)).
- **Push subscriptions** (M6b): per user and device, scoped to the caller and the org (`read` to list, `write` to change;
  other users' and tenants' ids are `404`). `POST` takes `PushSubscription.toJSON()` plus a `name` (1–100 characters,
  default `Device`); `endpoint` must be `https://`; `p256dh` must be a P-256 point and `auth` 16 bytes (base64url).
  Registering a known endpoint again (same org) replaces keys and name and answers `200`; an endpoint another member
  registered moves to the caller only with the same keys (same browser subscription), else `409 endpoint_taken`.
  Endpoint and keys are write-only (sealed); responses show `{id, name, host, created_at, updated_at}`. `DELETE`
  removes the device (tombstone, ciphertext wiped). Without a VAPID subject: `GET /push/config` → `{enabled: false}`,
  `POST` → `422 push_disabled`.
- **Silences:** all scope fields optional (none = the whole org); `selector` uses the rule selector syntax and is matched
  against resource labels; at most 365 days; `ends_at` must be in the future. `GET` lists only not-yet-ended silences unless
  `active=false`. `DELETE` sets `ends_at` to now (a future silence is removed).
- `GET /alerts/{id}` `notifications`: one entry per delivery job `{id, alert_event_id, channel_id, channel_name, device
  (web_push: the device name, also after removal; else null), status (queued|sent|failed), attempts, last_error,
  next_attempt_at, finished_at}`; the alert shows `acked_at`, `acked_by`, `last_notified_at`.
- Implemented error codes (schema `ErrorCode`): `unauthorized` (401), `invalid_credentials` (401, login), `forbidden` (403), `csrf_failed` (403), `wrong_password` (403), `too_many_attempts` (429), `not_found` (404, also for other tenants' ids and malformed ids),
  `bad_request` (400), `name_taken` (409), `sync_in_progress` (409), `default_rule` (409), `alert_resolved` (409), `invalid_token` (422, connect), `email_disabled` (422), `push_disabled` (422), `endpoint_taken` (409), `hetzner_invalid_token` / `hetzner_unavailable` (502),
  `hetzner_rate_limited` (503), `internal` (500).
