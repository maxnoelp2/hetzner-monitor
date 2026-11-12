# ADR-0013: Session cookie details, derived CSRF token, login throttling and master-key rotation

- Status: Proposed
- Date: 2026-11-12
- Deciders: Max Nölp

## Context

ADR-0007 decided on server-side sessions (no JWT), Argon2id and CSRF protection without fixing the details. ADR-0005
proposed versioned master keys and a `keys rotate` command. M5a implements both, and the M6 PWA builds on them.
Constraints: only hashes of credentials at rest (CLAUDE.md hard rules), one API process in the self-hosted case,
operators type secrets only into their own terminal, browsers on plain HTTP LAN addresses drop `Secure` cookies.

## Decision

**Sessions.** `POST /api/v1/auth/login` sets `hm_session=<32 random bytes, base64url>; Path=/; HttpOnly; SameSite=Lax;
Max-Age=2592000` plus `Secure` unless `HM_BASE_URL` starts with `http://`. Only `SHA-256(token)` is stored
(`sessions.token_hash`). A session ends after 30 days, after 14 days without use (`last_seen_at`, touched at most once a
minute), on logout, on a password change or reset (all other sessions of the user), when the user is disabled, or when the
membership is removed (FK cascade). Expired rows are purged on every successful login. A session's scope follows the
membership role: `owner`/`admin` → `admin`, `member` → `write`. An `Authorization` header always wins over the cookie.

**CSRF.** Cookie-authenticated requests with a method other than `GET`/`HEAD`/`OPTIONS` need
`X-CSRF-Token: base64url(SHA-256("hm-csrf:" || session token))`. The token is derived, not stored: every tab of a session
sees the same value (returned by login and `GET /me`), nothing extra sits in the database, and another origin cannot compute
it without reading the `HttpOnly` cookie. Bearer requests need no CSRF token (browsers never attach them automatically).

**Passwords.** Argon2id, PHC string, m=19 MiB, t=2, p=1 (OWASP baseline), 12–1024 characters, hashed in a blocking task.
Unknown e-mail, disabled user and missing password run a dummy verification and answer the same 401
`invalid_credentials`. Initial and reset passwords are **generated** and printed once by the CLI (`bootstrap`,
`users reset-password`) instead of read from a terminal (no new crate, nothing in shell history). Users who predate M5a
set a first password with an `admin` API key via `PUT /me/password`.

**Login throttling.** In-process sliding window of 15 minutes: 10 failures per e-mail (case-insensitive) or 30 per peer
address → 429 `too_many_attempts` with `Retry-After`. `X-Forwarded-For` is not trusted.

**Key rotation.** `HM_MASTER_KEYS=1:<b64>,2:<b64>` (or `HM_MASTER_KEYS_FILE`) is an alternative to
`HM_MASTER_KEY(_FILE)`, which means version 1; exactly one of the four may be set. New secrets use the highest version,
any listed version opens. `hetzner-monitor keys rotate` re-wraps every DEK not on the newest version (secret ciphertext
untouched, AAD unchanged, batches of 100 with `FOR UPDATE SKIP LOCKED` and a version guard, idempotent);
`keys status` counts rows per version. The service refuses to start when a stored `key_version` is not configured.

## Consequences

- **Pro:** No credential is recoverable from a database dump; CSRF needs no extra state; rotation needs neither Hetzner
  access nor plaintext secrets; a misconfigured key list fails loudly at startup instead of at the next poll.
- **Pro:** No new crates besides `argon2` (cookie parsing is a few lines).
- **Con:** Throttling is per process and, behind a reverse proxy, the per-address limit is shared by all clients (the
  per-e-mail limit still protects accounts). Multi-process (SaaS) throttling needs a shared store later.
- **Con:** A process started with an old key list cannot open rows rotated to a newer version: restart every process with
  the new list first, then rotate (runbook in `docs/design/security.md`).
- Rules out: JWT, CSRF tokens in the database, reading passwords from stdin. TOTP/OIDC stay post-MVP.
