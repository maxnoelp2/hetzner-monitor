# Design: `hcloud` client

Status: **accepted** ([ADR-0004](../adr/0004-own-minimal-hetzner-client.md)) · Crate: `crates/hcloud`

A small, hand-written client (`reqwest` + `serde`, rustls) for the Hetzner Cloud API.
Facts below are **from spec** as of 2026-11-05 ([OpenAPI spec](https://docs.hetzner.cloud/cloud.spec.json)).

## Scope — GET only

| Endpoint | Used by tier |
|---|---|
| `GET /servers` | fast |
| `GET /load_balancers` | fast |
| `GET /servers/{id}/metrics?type=cpu,disk,network` | metrics |
| `GET /load_balancers/{id}/metrics?type=…` | metrics |
| `GET /volumes`, `/certificates`, `/floating_ips`, `/primary_ips`, `/server_types`, `/pricing` | inventory |

Write endpoints are **out of scope forever** (invariant 1 in [ARCHITECTURE.md](../../ARCHITECTURE.md#invariants)).

## Behaviour

- **Auth:** `Authorization: Bearer <token>`. The token type is a wrapper that implements neither `Debug` nor `Display` with the real value.
- **Pagination:** read `meta.pagination` from the body (`page`, `per_page`, `next_page`, `last_page`, `total_entries`); always request `per_page=50` (maximum). **Do not use the `Link` header** — deprecated since the 2026-10-12 changelog entry.
- **Rate limit:** after every response, capture `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset` (UNIX ts) and hand them to the caller as part of the result.
- **Metrics:** values arrive as `[timestamp, "string"]` → parse to `(f64, f64)`. Max 500 samples per response; 30-day retention.
- **Errors:** response body `{ "error": { "code", "message", "details" } }`. Map at least:

| HTTP / code | Client error | Scheduler reaction |
|---|---|---|
| 401 `unauthorized` | `InvalidToken` | project → `invalid_token`, stop polling, notify owner |
| 403 `forbidden` | `Forbidden` | project → `degraded` |
| 403 `maintenance` | `Maintenance` | project → `degraded`, retry later |
| 429 `rate_limit_exceeded` | `RateLimited { reset_at }` | sleep until `reset_at` |
| 5xx / network | `Unavailable` | backoff, `degraded` after N failures |

- **Timeouts:** connect 5 s, total 20 s (proposed). Retries are the scheduler's job, not the client's.

## Testing

- Record real responses from a dedicated test project, **anonymise** (IDs, IPs, names, labels), store under `crates/hcloud/tests/fixtures/`.
- Replay with `wiremock`. Include pagination, 401, 429 and malformed-metric cases.
