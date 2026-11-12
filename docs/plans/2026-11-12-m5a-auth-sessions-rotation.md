# Plan: M5a — login, server-side sessions, master-key rotation

- Status: In progress (implemented, PR open; owner real check pending)
- Milestone: M5 (part a of two)
- Owner: agent (Claude Code), reviewed by Max Nölp
- Started: 2026-11-12
- PR: —

## Split of M5

M5 ("hardening: key rotation, login + sessions, OpenAPI, docs, release v0.1.0") is two independent concerns, so it is
split into two PRs:

- **M5a (this plan):** password login, server-side sessions with CSRF protection, `/me`, password management,
  master-key rotation (`HM_MASTER_KEYS`, `hetzner-monitor keys rotate`). Everything the M6 PWA needs to log in.
- **M5b (own plan, after M5a is merged):** OpenAPI via `utoipa` 6 + `utoipa-axum` 0.3 at `/api/v1/openapi.json` for all
  endpoints, self-hosting guide in the README, release workflow (tag `v*` → multi-arch image on GHCR), version
  `0.1.0`, `CHANGELOG.md`. Acceptance: "self-hostable from the README alone".

OpenAPI comes after auth because annotating every handler is mechanical and should include the new auth endpoints
and the cookie/CSRF security schemes from the start.

## Goal

A user can log in with e-mail and password, gets a revocable `HttpOnly` session cookie, and can use the whole API with it
(state-changing requests need a CSRF token). An operator can add a new master key and re-wrap all stored DEKs with it
without touching the secrets themselves or calling Hetzner.

## Acceptance criteria

- [x] `POST /api/v1/auth/login {email, password}` → 200 `{user, org, role, csrf_token}` + `Set-Cookie: hm_session=…;
      HttpOnly; SameSite=Lax; Path=/; Secure` (Secure unless `HM_BASE_URL` is `http://`); wrong e-mail or password → the same
      401 `invalid_credentials` with comparable timing (dummy Argon2 verify for unknown users); disabled users cannot log in.
- [x] Login rate limit: 10 failures per e-mail or 30 per client address within 15 min → the next login gets 429
      `too_many_attempts` with `Retry-After` (in-process counter, see Risks).
- [x] Every existing endpoint accepts either the session cookie or the Bearer API key. Cookie requests with a method other
      than `GET`/`HEAD`/`OPTIONS` need `X-CSRF-Token` matching the session → otherwise 403 `csrf_failed`. Bearer requests are unchanged.
- [x] Session scope follows the membership role: `owner`/`admin` → `admin`, `member` → `write`.
- [x] `POST /auth/logout` deletes the session and clears the cookie. `GET /me` → `{user, org, role, auth: "session"|"api_key", csrf_token?}`.
- [x] Sessions: only SHA-256 of the token stored (`sessions.token_hash`), idle timeout 14 days (sliding, `last_seen_at`
      touched at most once per minute), absolute lifetime 30 days; expired rows are purged on every successful login.
- [x] `PUT /me/password {current_password, new_password}` (session or key) — new password 12–1024 chars; ends all other
      sessions of the user. Passwords stored as Argon2id PHC strings (OWASP parameters m=19 MiB, t=2, p=1), hashed in
      `spawn_blocking`.
- [x] Getting a first password without the chat or shell history: `bootstrap` now also prints a generated initial password
      once (next to the admin key); `hetzner-monitor users reset-password --email <e>` generates a new one, prints it
      once and ends the user's sessions.
- [x] Key rotation: `HM_MASTER_KEYS=1:<b64>,2:<b64>` (or `HM_MASTER_KEYS_FILE`) as an alternative to `HM_MASTER_KEY(_FILE)`
      (which keeps meaning "version 1"); new secrets are sealed with the highest version; reading works with every listed
      version. `hetzner-monitor keys rotate` re-wraps the DEKs of all project tokens and channel configs to the newest
      version (secrets untouched, no Hetzner call) and prints counts; `hetzner-monitor keys status` prints rows per version.
- [x] Startup refuses to run if the database holds a `key_version` that the configured keys do not contain (clear error
      naming the version, no key material).
- [x] No secret in logs, errors or responses: password, session token, CSRF token (only returned once in login / `/me`).
- [x] Tests: unit (keyring parsing, rewrap, password hash/verify, cookie parsing, rate limiter) and integration
      (login/logout/me, CSRF, scope by role, expiry, password change ends sessions, rotation end-to-end incl. decrypt
      after rotate, startup check, cross-tenant: a session never sees another org's data).
- [x] Docs: `security.md` (sessions, CSRF, rate limit, rotation runbook → accepted), `api.md`, `configuration.md`,
      `data-model.md`, `ARCHITECTURE.md` status, `CLAUDE.md` status/commands, ADR-0013, roadmap.
- [x] fmt + clippy clean, all tests green; `scripts/smoke-m5a.sh` for the owner's real check (local stack only).
- [ ] Owner's real check on the local stack (`scripts/smoke-m5a.sh`, `keys status`).

## Context

Read and verified on 2026-11-12:

- ADR-0005 (envelope encryption, rotation "proposed"), ADR-0007 (sessions + API keys, Argon2id, no JWT),
  `docs/design/security.md`, `api.md`, `configuration.md`, roadmap.
- `hm_core::crypto`: one `MasterKey`, `CURRENT_KEY_VERSION = 1`, `open` rejects other versions. Sealed rows:
  `hetzner_projects (token_ct, token_dek_ct, key_version)`, `notification_channels (config_ct, config_dek_ct, key_version)`;
  the notifier reads the channel ciphertext through its job join. AAD = row id, so re-wrapping keeps the AAD.
- `users.password_hash` (text, nullable) already exists in `0001_init.sql`; bootstrap creates the owner without a password.
- `auth::Principal { org, user, key_id, scope }` — `key_id` is only set, never read elsewhere.
- Crates (crates.io, 2026-11-12): `argon2` 0.6.0 (RustCrypto, pure Rust, `password-hash` PHC strings, `zeroize` feature).
  `utoipa` 6.0.0 / `utoipa-axum` 0.3.0 (axum ^0.8.4) for M5b. GitHub actions for M5b: `docker/login-action` v4,
  `metadata-action` v6, `build-push-action` v7, `setup-buildx-action` v4, `setup-qemu-action` v4.

## Approach

1. [x] **ADR-0013** "Session cookie, CSRF token and login throttling; master-key rotation" — records the concrete
   choices below (cookie name/flags, synchronizer CSRF token in a header instead of double-submit, role→scope mapping,
   TTLs, in-process throttling, keyring format, startup version check, generated passwords instead of reading them from
   a terminal). ADR-0005 rotation and ADR-0007 stay accepted; security.md rotation becomes accepted.
2. [x] **Keyring** in `hm_core::crypto`: `Keyring { keys: BTreeMap<i32, MasterKey> }`, `current()`, `seal` with the
   current version, `open` by the row's version, `rewrap(keyring, aad, sealed) -> Sealed` (unwrap DEK with its version,
   wrap with current; `secret_ct` unchanged). `AppState.master_key` becomes `keyring: Arc<Keyring>`.
3. [x] **Config**: exactly one of `HM_MASTER_KEY`, `HM_MASTER_KEY_FILE`, `HM_MASTER_KEYS`, `HM_MASTER_KEYS_FILE`;
   `version:base64` list, versions 1–32767, unique; errors never echo key material.
4. [x] **Rotation CLI**: `keys status` and `keys rotate` (batches of 100 rows per table in a transaction,
   `UPDATE … WHERE id = $1 AND key_version = $old` so a concurrent write is skipped and picked up by the next run;
   idempotent). Startup check `SELECT DISTINCT key_version` over both tables.
5. [x] **Migration `0007_sessions.sql`**: `sessions (id, user_id, org_id, token_hash bytea unique, created_at,
   last_seen_at, expires_at, user_agent text null)` (CSRF token derived, see progress log) with FKs on user and membership org; index on
   `user_id`, `expires_at`.
6. [x] **Passwords** `hm_core::password`: `hash`, `verify` (Argon2id, PHC), `generate` (24 chars base64url from OS
   RNG), dummy hash for unknown users. New crate: `argon2` (justification: ADR-0007 mandates Argon2id; RustCrypto,
   no C, no OpenSSL).
7. [x] **Auth extractor**: `Principal` gets `credential: Credential::{ApiKey(id), Session(id)}`; Bearer first, then
   `Cookie: hm_session` (own small parser, no new crate); CSRF check for unsafe methods on cookie auth; session touch.
   401 message becomes "missing or invalid credentials".
8. [x] **Endpoints** `/auth/login`, `/auth/logout`, `/me`, `PUT /me/password`; login throttle in `AppState`
   (`Arc<Mutex<…>>`, keyed by e-mail and peer address via `ConnectInfo`, falls back to e-mail only when absent, e.g.
   in tests); serve with `into_make_service_with_connect_info`.
9. [x] **CLI**: bootstrap prints an initial password; `users reset-password --email`.
10. [x] **Purge** expired sessions on every successful login (works in `HM_MODE=api` too).
11. [x] Tests (unit + integration, see acceptance criteria); `tests/common` gets `login()` helper.
12. [x] Docs (see acceptance criteria), `scripts/smoke-m5a.sh` (macOS Python 3.9 compatible helpers, reads password
    from the environment, never echoes it): login → `/me` → CSRF-protected POST → logout; `keys status`.

## Out of scope

- OpenAPI, README self-hosting guide, release/GHCR, v0.1.0 → **M5b**.
- Inviting further users, user management API, multiple orgs per session switch, TOTP, OIDC, "remember me" variants.
- Distributed (multi-process) login throttling — the SaaS concern; self-hosted runs one API process.
- Re-encrypting secrets themselves (only DEKs are re-wrapped, per ADR-0005).

## Risks / open questions

- **Throttling behind a reverse proxy:** the peer address is the proxy, so the per-address limit is shared by all
  clients; the per-e-mail limit still protects accounts. No `X-Forwarded-For` trust in M5a (spoofable without a
  configured trusted proxy); revisit with M5b's deployment docs.
- **Rotation while an old process runs:** a process that only knows version 1 cannot open rows re-wrapped to version 2.
  Runbook: restart every process with the new key list first, then run `keys rotate`, then (optionally) drop the old key
  after `keys status` shows zero rows for it.
- **Secure cookie on plain-HTTP local setups:** browsers accept `Secure` cookies on `http://localhost`, but not on a
  LAN IP over HTTP; hence Secure is dropped only when `HM_BASE_URL` is explicitly `http://`.
- **Multiple memberships:** login picks the user's oldest membership; an optional `org_id` in the login body selects
  another. Enough until user management exists.

## Progress log

- 2026-11-12 — Plan drafted; M5 split into M5a (this) and M5b (OpenAPI, docs, release). Approved by the owner.
- 2026-11-12 — Implemented. Changes against the draft, recorded in ADR-0013:
  - **CSRF token is derived** from the session token (`SHA-256("hm-csrf:" || token)`) instead of a stored hash: nothing
    extra at rest, the same value in every tab, and `GET /me` can return it again (a stored hash could not be returned).
    The `sessions.csrf_hash` column was dropped from the draft migration.
  - **Session purge on login** instead of in the scheduler: the scheduler does not run with `HM_MODE=api`.
  - `PUT /me/password` without a stored password needs an `admin` **API key** (migration path for owners bootstrapped
    before M5a); wrong current password → 403 `wrong_password`.
  - The e-mail lookup must cast to `citext` (`$1::citext`); comparing `citext = text` is case-sensitive.
  - Argon2 and blake2 are built with `opt-level = 3` in the dev profile, otherwise tests spend seconds per hash.
  - Smoke script verified against a throwaway instance (port 18080, own database): login, `/me`, CSRF refusal and
    acceptance, logout; `keys status`/`keys rotate` CLI output checked. The owner's stack was not touched.
