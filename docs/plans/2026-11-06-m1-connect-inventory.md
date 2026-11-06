# Plan: M1 — hcloud client, bootstrap, API keys, connect project, inventory sync

- Status: In progress — implementation done, waiting for the owner's real-project smoke test
- Milestone: M1
- Owner: Claude (agent), reviewed by Max Nölp
- Started: 2026-11-06
- PR: —

## Goal

An operator can bootstrap an instance, authenticate with an API key, connect a real Hetzner Cloud
project with a read-only token (stored envelope-encrypted), and read its inventory through the API.
No scheduler, rules or alerts yet — sync runs on connect and on demand.

## Acceptance criteria

- [x] `hetzner-monitor bootstrap --org <name> --email <email>` on an empty DB creates org + owner + an `admin` API key, prints the key **once** to stdout; a second run refuses.
- [x] Without a valid `HM_MASTER_KEY` / `HM_MASTER_KEY_FILE` the service refuses to start with a clear error; `hetzner-monitor master-key generate` prints a fresh key.
- [x] Requests without / with an invalid / with a revoked API key get `401`; a `read` key gets `403` on write endpoints.
- [x] `POST /api/v1/projects {name, token}` with a valid token → `201`, initial inventory sync done; with an invalid token → `422 invalid_token`, nothing stored.
- [x] `hetzner_projects.token_ct` never contains the plaintext token (test inspects the row); API responses only show `token_hint` (last 4 chars).
- [x] `GET /api/v1/projects/{id}/resources` lists servers, load balancers, volumes, certificates, floating IPs, primary IPs with `name`, `labels`, `first_seen_at`, `last_seen_at`, `deleted_at`.
- [x] `POST /api/v1/projects/{id}/sync` re-syncs: new resources appear, vanished ones get `deleted_at`, reappearing ones are revived.
- [x] **Tenant isolation:** a key of org A gets `404` for org B's project, resources and sync.
- [x] `RateLimit-*` values from the last Hetzner response are stored on the project and shown in `GET /projects/{id}`.
- [ ] **Real-project smoke test** (`scripts/smoke-m1.sh`, run by Max with his own read token): connect → inventory matches the Hetzner Console; token appears nowhere in logs or output.
- [x] fmt, clippy `-D warnings`, all tests green locally and in CI; no OpenSSL; docs updated.

## Context

Read: `docs/roadmap.md` (M1), `ARCHITECTURE.md` (invariants 1–5), ADR-0004 (own client), ADR-0005 (envelope
encryption), ADR-0006 (tenancy), ADR-0007 (API keys), `docs/design/{hcloud-client,security,data-model,api,configuration}.md`,
`docs/research/hetzner-cloud-api.md`. Code after M0: `hm_core::secret::Secret`, `hm_server::{config,db,http,telemetry}`,
`migrations/0001_init.sql`.

Verified 2026-11-06: crates.io versions (below); `ring 0.17` is already in the tree via `sqlx → rustls`.
Hetzner facts from the [OpenAPI spec](https://docs.hetzner.cloud/cloud.spec.json) as recorded in the research doc (2026-11-05).

## Approach

1. [x] **Crypto (`hm-core::crypto`)** — `MasterKey` (32 bytes, base64, wrapped in `Secret`, zeroized on drop),
       `seal(master, aad, plaintext) -> Sealed { secret_ct, dek_ct, key_version }` and `open(...)` per
       [security §envelope](../design/security.md#envelope-encryption-for-secrets-at-rest): random DEK per row,
       AES-256-GCM, random 96-bit nonces prefixed to ciphertext, AAD = row UUID. Only `key_version = 1` in M1
       (rotation is M5). Unit tests: round trip, wrong AAD / wrong key / tampered bytes fail, no plaintext in output.
2. [x] **Config** — `HM_MASTER_KEY` or `HM_MASTER_KEY_FILE` (exactly one, required for `serve`), validated length.
       CLI with `clap`: `serve` (default), `healthcheck`, `bootstrap`, `master-key generate`.
3. [x] **Migration `0002_projects_api_keys.sql`** — `api_keys`, `hetzner_projects`, `resources` as in
       [data-model](../design/data-model.md), with `CHECK`s on enums, `UNIQUE(prefix)` on API keys,
       `UNIQUE(project_id, kind, hetzner_id)`, FK `ON DELETE CASCADE` from resources. `sessions` stays for M5.
4. [x] **API keys** — format `hm_<12-char prefix>_<32 random bytes, base64url>`; store `prefix` + SHA-256 of the full key;
       lookup by prefix, constant-time hash compare (`subtle`); scopes `read | write | admin`; `last_used_at`
       updated at most once per minute. Axum extractor `Principal { user_id, org_id, scopes }`; errors `401` / `403`.
       Endpoints: `GET/POST /api/v1/api-keys`, `DELETE /api/v1/api-keys/{id}` (admin scope; full key returned once on create).
5. [x] **Bootstrap CLI** — `hetzner-monitor bootstrap --org <name> --email <email>`: in one transaction, only if
       `users` is empty → org, user, `owner` membership, `admin` API key; print key once to stdout with a "store it now" note.
       **Changes the documented bootstrap** (env vars `HM_ADMIN_EMAIL/PASSWORD` → CLI, no password until login lands in M5);
       update `configuration.md` and `data-model.md`.
6. [x] **`hm-hcloud` client** — `reqwest` 0.13 with `rustls-no-provider` + ring provider, `json`; base URL injectable
       (tests); `User-Agent: hetzner-monitor/<version>`; timeouts connect 5 s / total 20 s. `GET`-only generic
       `list_all(path, key)` following `meta.pagination` with `per_page=50`; returns items as
       `RawResource { id, name, labels, raw: serde_json::Value }` plus `RateLimit { limit, remaining, reset_at }`
       of the last response. Error enum per [hcloud-client](../design/hcloud-client.md#behaviour) (401, 403, 403 maintenance,
       429 with reset, 5xx/network, decode). Token as `&Secret<String>`, never in `Debug`/errors.
       Endpoints in M1: `servers`, `load_balancers`, `volumes`, `certificates`, `floating_ips`, `primary_ips`. Metrics: M3.
7. [x] **Repositories (`hm-server::repo`)** — every function takes `OrgId` (newtype) first; queries join/filter on `org_id`.
       Projects: insert (sealed token, hint, status), list, get, delete. Resources: upsert batch + mark-deleted in one transaction.
8. [x] **Sync service** — decrypt token in memory → fetch the six kinds → upsert (`first_seen_at` kept, `last_seen_at` = now,
       `deleted_at` cleared), mark missing as deleted → store `rl_*` and `last_inventory_at`. Map errors to project
       `status`: 401 → `invalid_token`, 403/5xx/network → `degraded`, success → `active`. No retries here (scheduler, M2).
9. [x] **Project endpoints** — `GET/POST /api/v1/projects`, `GET/DELETE /api/v1/projects/{id}`,
       `POST /api/v1/projects/{id}/sync`, `GET /api/v1/projects/{id}/resources?kind=`. Write endpoints need `write` scope.
       Connect = validate token with one `GET /servers?per_page=1`, then insert + initial sync.
       Error shape `{ "error": { "code", "message" } }` per [api](../design/api.md#conventions).
10. [x] **Tests** — `hm-core`: crypto. `hm-hcloud`: `wiremock` with hand-written, anonymised fixtures shaped after the spec
        (pagination over 2 pages, 401, 403 maintenance, 429, malformed body, rate-limit headers). `hm-server` integration
        (`#[sqlx::test]` + wiremock Hetzner): auth matrix, connect valid/invalid token, ciphertext ≠ plaintext,
        sync create/delete/revive, tenant isolation across two orgs, bootstrap idempotency.
11. [x] **Smoke script** — `scripts/smoke-m1.sh`: reads `HCLOUD_TOKEN` and `HM_API_KEY` from the environment
        (never as arguments, never echoed), connects a project against the local stack, prints resource counts per kind.
12. [x] **Docs** — CLAUDE.md (status, bootstrap/smoke commands), configuration (`HM_MASTER_KEY*`), data-model (as built),
        api (implemented endpoints), hcloud-client (ring provider, `RawResource`), roadmap ticks, this plan.
13. [ ] **Verify** — all acceptance criteria; Max runs the smoke test against his test project; record results; open PR.

## Dependencies (new crates — justification)

| Crate | Version | Why |
|---|---|---|
| `reqwest` (`rustls-no-provider`, `json`) | 0.13.5 | HTTP client for Hetzner (ADR-0004); ring provider shared with sqlx — one crypto stack |
| `aes-gcm` | 0.11.1 | AES-256-GCM for envelope encryption (ADR-0005), RustCrypto |
| `getrandom` | 0.4.3 | OS randomness for DEKs, nonces, API keys (no `rand` needed) |
| `zeroize` | 1.9.1 | wipe keys/plaintext tokens from memory |
| `base64` | 0.23.1 | master key and API key encoding |
| `sha2` | 0.11.1 | API key hashing |
| `subtle` | 2.6.1 | constant-time hash comparison |
| `uuid` (+ sqlx `uuid`) | 1.29.0 | typed IDs |
| `time` (+ sqlx `time`, `serde`) | 0.3.55 | `timestamptz` ↔ Rust, UTC |
| `clap` (derive) | 4.6.7 | CLI subcommands with arguments (`bootstrap`, `master-key generate`) |
| `wiremock` (dev) | 0.6.5 | replay Hetzner responses in tests |

Exact patch versions re-checked when added. No `argon2` yet — passwords arrive with login in M5.

## Out of scope

- Scheduler, polling tiers, budget enforcement, retries (M2) — M1 only records `RateLimit-*`.
- Typed resource models and all checks (M2/M3); metrics endpoints (M3).
- Login, sessions, passwords, CSRF (M5); master-key rotation (M5); OpenAPI generation (M5).
- Recording real Hetzner fixtures automatically — fixtures are hand-written from the spec; the smoke test checks against reality.

## Risks / open questions

- **Bootstrap via CLI instead of env vars** (step 5) changes a documented proposal — needs Max's OK.
- **Real test project needed:** Max needs a Hetzner Cloud project (ideally a dedicated test project with 1–2 small servers) and a **Read** token. The token is only ever typed into his own terminal for the smoke script, never into chat or files.
- Hand-written fixtures may drift from reality → the smoke test is the safety net; if it reveals differences, anonymise and add the real payload as a fixture.
- `reqwest` 0.13 provider setup (`rustls-no-provider` + ring) — verify that TLS to `api.hetzner.cloud` works in the distroless image (CA roots via `rustls-platform-verifier`); fall back to webpki roots if not.
- Deleting a project deletes its resources (cascade) — acceptable while nothing else references them.

## Result (verified 2026-11-06)

| Criterion | Result |
|---|---|
| Bootstrap once, key printed once, second run refused | ✅ test `bootstrap_runs_once` + by hand in the Docker image |
| No master key → refuses to start; `master-key generate` | ✅ `error: required environment variable HM_MASTER_KEY is not set`; generate prints 44-char base64 |
| Auth 401 / 403 matrix, revoke | ✅ test `auth_matrix` |
| Connect valid → 201 + sync; invalid → 422, nothing stored | ✅ tests `connect_stores_sealed_token_and_syncs`, `invalid_token_is_rejected_and_nothing_is_stored` |
| Ciphertext ≠ plaintext, opens with row id as AAD; only `token_hint` returned | ✅ same test inspects the row |
| Resources list incl. kind filter | ✅ |
| Re-sync: new / deleted / revived | ✅ test `resync_tracks_new_deleted_and_revived` |
| Tenant isolation (projects, resources, sync, delete, API keys) | ✅ test `tenants_are_isolated` |
| `RateLimit-*` stored and shown | ✅ `rl_limit` 3600 / `rl_remaining` 3500 from mock headers |
| TLS to the real API from the distroless image | ✅ fake token → `422 invalid_token` from `api.hetzner.cloud`; also `cargo test -p hm-hcloud -- --ignored live_` |
| No secrets in logs | ✅ app logs contain neither the fake Hetzner token nor `hm_` keys |
| fmt / clippy `-D warnings` / tests / no OpenSSL / no aws-lc-rs | ✅ 37 tests (+1 ignored live test) |
| **Real-project smoke test** | ⏳ owner runs `scripts/smoke-m1.sh` |

## Progress log

- 2026-11-06 — Plan drafted. Decided to share `ring` with sqlx instead of reqwest's default `aws-lc-rs`; defer passwords/argon2 to M5; bootstrap via CLI.
- 2026-11-06 — Implemented. Deviations: (1) `hm-hcloud` has its own redacted `Token` type — the crate may not depend on `hm-core` (ARCHITECTURE dependency rule). (2) sqlx 0.9 rejects dynamic SQL strings; shared column lists use `concat!` + a macro. (3) `api_keys.revoked_at` added (revocation keeps the row for audit). (4) `hetzner_projects.status_message` added for the last Hetzner error. (5) Workspace allows `clippy::must_use_candidate` (pedantic noise). (6) Compose no longer enforces `HM_MASTER_KEY` via `${…:?}` — that broke `down`/`logs`; the app itself refuses to start without it. (7) reqwest built with `default-features = false` + `rustls-no-provider` + ring: verified no `aws-lc-rs` in the tree.
- 2026-11-06 — Verified by hand in Docker: healthy, bootstrap, second bootstrap refused, 401 without key, 200 with key, live TLS to Hetzner (fake token → 422). Stack torn down with `-v` so the owner starts fresh.
