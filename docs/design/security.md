# Design: Security

Status: encryption & auth **accepted** ([ADR-0005](../adr/0005-envelope-encryption-for-secrets.md), [ADR-0007](../adr/0007-auth-sessions-and-api-keys.md)); key rotation **proposed**

## Threat model (short)

The database holds API tokens for other people's infrastructure. Assume: DB dumps leak, logs get
shared in bug reports, webhook targets are attacker-controlled (SaaS). Protect tokens first.

## Envelope encryption for secrets at rest

Applies to Hetzner tokens and notification channel configs (Slack/Discord webhook URLs are secrets too).

1. Generate a random 256-bit **DEK** per row.
2. `secret_ct = AES-256-GCM(DEK, nonce₁, secret, AAD = row id)`
3. `dek_ct = AES-256-GCM(master_key[key_version], nonce₂, DEK, AAD = row id)`
4. Store `secret_ct`, `dek_ct`, `key_version`. Nonces are random 96-bit and prefixed to the ciphertext.
5. AAD binds a ciphertext to its row — swapping ciphertexts between rows fails decryption.

**Master key:** 32 bytes, base64, from `HM_MASTER_KEY` or `HM_MASTER_KEY_FILE` (Docker secret).
The service **refuses to start** without it. Lose it → all stored tokens are unrecoverable (document this for operators).

**Rotation (proposed):** `HM_MASTER_KEYS` holds versioned keys; `hetzner-monitor keys rotate` re-wraps all DEKs
with the newest version. Secrets themselves are not re-encrypted; no Hetzner access needed.
In the SaaS, step 3 moves to a KMS/Vault.

## Token handling

- Decrypt only in memory, right before use; zeroize after (`zeroize` crate).
- Never returned by the API — only `token_hint` (last 4 chars).
- On connect: validate with one `GET`. Write permission cannot be detected without a write call, so the UI must state clearly that a **Read** token is sufficient and recommended.

## Authentication

- **PWA:** session cookie (`HttpOnly`, `Secure`, `SameSite=Lax`), sessions stored server-side (`sessions.token_hash`) so they are revocable. **No JWT.** CSRF protection for state-changing requests.
- **Headless / CLI / Tauri:** API keys `hm_<prefix>_<secret>` as `Authorization: Bearer`, scopes `read | write | admin`. Only a SHA-256 hash is stored; the full key is shown once.
- Passwords: Argon2id. Login rate limiting. TOTP/OIDC after MVP.

## Authorization

Role per membership: `owner | admin | member`. Every handler resolves the caller's `org_id` first and passes it to repositories (tenant isolation, invariant 5).

## Outbound requests (SSRF)

Webhook/ntfy URLs are user-controlled. With `HM_ALLOW_PRIVATE_TARGETS=false` (to be the SaaS default) the notifier refuses
targets in private, loopback, link-local (incl. `169.254.169.254`), CGNAT, unspecified, multicast, reserved and
unique-local ranges, also IPv4-mapped and NAT64 forms: IP literals are checked directly, host names through a DNS resolver
that drops such addresses for every connection (so rebinding between check and connect does not help). Redirects are never
followed and proxies are not used. Self-hosted default: `true` (alerts to a LAN ntfy or webhook are common).
Implemented in `hm_notify::guard` (M4).

## Redaction

- Secret types implement neither `Debug` nor `Display` with real content.
- `tracing` fields for tokens/URLs/keys are redacted. Error messages never embed secrets.
- Channel configs (`hm_notify::ChannelConfig`), `SmtpSettings` and the notifier print only their type; delivery errors are
  short fixed strings (`HTTP 404`, `timeout`, …) built without the URL, so `last_error` and the test endpoint never leak
  a webhook URL, token, secret or recipient. Configs are sealed with AAD = channel id.
- Reported security issues: see [SECURITY.md](../../SECURITY.md).
