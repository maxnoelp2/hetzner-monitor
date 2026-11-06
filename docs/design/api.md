# Design: HTTP API

Status: **proposed** · Crate: `crates/server`

REST under `/api/v1`, JSON. The OpenAPI document is **generated from code** (`utoipa`) and served at
`/api/v1/openapi.json`; the PWA's TypeScript client is generated from it.

```
POST   /auth/login · POST /auth/logout · GET /me
GET    /projects · POST /projects {name, token} · GET/PATCH/DELETE /projects/{id}
POST   /projects/{id}/test           → validate token, show rate-limit status
GET    /projects/{id}/resources      → inventory
GET    /resources/{id}/metrics       → proxy to Hetzner metrics (for later graphs; budgeted)
GET    /rules · POST /rules · PATCH/DELETE /rules/{id}
GET    /alerts?state=firing · POST /alerts/{id}/ack
GET    /channels · POST /channels · POST /channels/{id}/test
GET    /silences · POST /silences · DELETE /silences/{id}
GET    /api-keys · POST /api-keys · DELETE /api-keys/{id}
GET    /healthz · GET /readyz · GET /metrics (Prometheus, own operational metrics; unauthenticated only if bound to localhost/internal)
```

## Conventions

- Auth: session cookie or `Authorization: Bearer hm_…` ([security](security.md#authentication)).
- Errors: `{ "error": { "code": "snake_case", "message": "…" } }` with proper HTTP status.
- Pagination: `?page=&per_page=` (max 100), response `meta.pagination` — same shape as Hetzner's.
- IDs are UUIDs; Hetzner IDs appear only as `hetzner_id`.
- Secrets are write-only: accepted on create/update, never returned.
