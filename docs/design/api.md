# Design: HTTP API

Status: endpoints marked ✅ are **implemented** (M1); the rest **proposed** · Crate: `crates/server`

REST under `/api/v1`, JSON. The OpenAPI document is **generated from code** (`utoipa`) and served at
`/api/v1/openapi.json`; the PWA's TypeScript client is generated from it.

```
POST   /auth/login · POST /auth/logout · GET /me
✅ GET  /projects · ✅ POST /projects {name, token} → 201 {project, sync} · ✅ GET/DELETE /projects/{id} · PATCH /projects/{id}
✅ POST /projects/{id}/sync           → re-sync inventory now → {sync, project}
✅ GET  /projects/{id}/resources?kind= → inventory (server|load_balancer|volume|certificate|floating_ip|primary_ip)
GET    /resources/{id}/metrics       → proxy to Hetzner metrics (for later graphs; budgeted)
GET    /rules · POST /rules · PATCH/DELETE /rules/{id}
GET    /alerts?state=firing · POST /alerts/{id}/ack
GET    /channels · POST /channels · POST /channels/{id}/test
GET    /silences · POST /silences · DELETE /silences/{id}
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
- Implemented error codes: `unauthorized` (401), `forbidden` (403), `not_found` (404, also for other tenants' ids and malformed ids),
  `bad_request` (400), `name_taken` (409), `invalid_token` (422, connect), `hetzner_invalid_token` / `hetzner_unavailable` (502),
  `hetzner_rate_limited` (503), `internal` (500).
