# ADR-0007: Server-side sessions for the PWA, API keys for headless use

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Clients: browser PWA, CLI/scripts, later Tauri apps. Needs revocation and simple security properties.

## Decision

PWA uses an `HttpOnly`/`Secure`/`SameSite=Lax` session cookie with sessions stored in Postgres — no JWT. Headless clients use scoped API keys (`hm_<prefix>_<secret>`) as Bearer tokens; only hashes are stored. Passwords use Argon2id.

## Consequences

- **Pro:** Instant revocation, no token-expiry juggling, small attack surface.
- **Con:** CSRF protection needed for cookie auth. TOTP/OIDC deferred to after MVP.
