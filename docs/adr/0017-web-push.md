# ADR-0017: Web Push with our own RFC 8291/8292 code, sealed VAPID keys and one job per device

- Status: Proposed
- Date: 2026-11-14
- Deciders: Max Nölp (owner), Claude (agent, draft)

## Context

M6 must deliver an alert as a push notification on a phone ([roadmap](../roadmap.md), M6). Browsers deliver Web Push
(RFC 8030) through their vendor's push service (Google FCM, Mozilla autopush, Apple `web.push.apple.com`); the
application server must encrypt the message for the browser (RFC 8291, `aes128gcm`) and identify itself with a VAPID
token (RFC 8292, ES256 JWT). Constraints, checked on 2026-11-13/14:

- CI forbids OpenSSL in the dependency tree. The `web-push` crate (0.11) pulls in `ece` with its OpenSSL backend and
  `isahc`/`hyper-tls`, so it is not usable.
- Secrets at rest are envelope-encrypted ([ADR-0005](0005-envelope-encryption-for-secrets.md)). A VAPID private key is a secret; a
  push subscription's endpoint is a capability (whoever knows it can push to the device), so it is one too.
- Notifications are delivered at least once from a Postgres queue, one job per alert event and channel
  ([ADR-0012](0012-notification-delivery.md)). A user may have several devices, and a device can disappear at any time
  (the push service answers `404`/`410`).
- iOS/iPadOS (16.4+) support Web Push only for web apps added to the home screen, and Apple's push service requires a
  VAPID `sub` claim. Web Push and service workers need a secure context (HTTPS or `localhost`).

## Decision

- **Own implementation in `hm-notify`** (`push.rs`, about 300 lines with tests) on RustCrypto: `p256` (ECDH, ECDSA),
  `hkdf`, plus `aes-gcm`, `sha2` and `base64` that are already dependencies. RFC 8291 single-record `aes128gcm`
  (record size 4096, plaintext ≤ 3072 bytes) and RFC 8292 `Authorization: vapid t=<JWT>, k=<key>` with a 12-hour
  `exp`. A unit test reproduces the RFC 8291 test vector byte for byte. Messages are sent over the existing guarded
  `reqwest` client (SSRF guard, no redirects, timeouts) with `TTL` and `Urgency` set by severity.
- **One VAPID key pair per installation**, generated on first use (at startup when push is enabled) and stored in
  `vapid_keys` sealed like every other secret (AAD = row id), covered by `keys status` / `keys rotate`. No key
  variables in the environment. `HM_VAPID_SUBJECT` (`mailto:` or `https:`) sets the `sub` claim; without it an
  `https://` `HM_BASE_URL` is used; with neither, push is disabled (`422 push_disabled`, `GET /push/config` says so).
- **Push subscriptions per user and device** in `push_subscriptions`: endpoint and keys sealed (AAD = row id); the API
  shows only a name, the push service host and timestamps. Registration is idempotent per endpoint within an org (a
  SHA-256 of the endpoint is the lookup key). Removed or expired subscriptions become tombstones (`deleted_at`,
  ciphertext wiped) so the delivery history stays readable; they are purged after the job retention period.
- **`web_push` is a channel type** routed like any other channel (`min_severity`, rule pins, silences). Its config is
  `{users?: [user_id]}` (omitted = every member), copied into the plain column `notification_channels.push_users` so
  the fan-out can run inside the alert transaction without decrypting. **At enqueue time a `web_push` channel fans out
  into one job per live subscription** (`notification_jobs.push_subscription_id`, unique per event, channel and
  device). A resolve goes to the devices that got the fired/renotified notification and still exist.
- **`404`/`410` from the push service** is a new send outcome (`Gone`): the job fails without retry and the
  subscription is tombstoned in the same transaction. Channel tests send to every targeted device and report each.
- **Service worker** (`web/public/sw.js`): `push` shows the notification (title, text, one tag per alert with
  `renotify`, `requireInteraction` for critical); `notificationclick` focuses or opens the app at `/alerts/{id}`. Only
  same-origin paths are opened.

## Consequences

- **Pro:** no OpenSSL, no new HTTP stack; the crypto is small, tested against the RFC vector, and was checked end to
  end against Google's push service (FCM accepted the VAPID token, Chrome decrypted and showed the message, an
  unsubscribed browser produced `410` and was removed).
- **Pro:** a dead device never makes a job for other devices retry or re-send; delivery status per device is visible
  on `GET /alerts/{id}` (`device`).
- **Pro:** nothing to generate or back up by hand; the VAPID key rotates with the master key like other secrets.
- **Con:** we maintain crypto code ourselves (mitigated by the RFC test vector and the end-to-end check). Changing the
  VAPID key (e.g. by deleting the row) silently invalidates every subscription; browsers must subscribe again.
- **Con:** `push_users` duplicates the non-secret part of a sealed config; both are written together.
- **Con:** self-hosters need HTTPS for phones (README: Tailscale Serve or a reverse proxy); iOS users must add the app
  to the home screen first.
- Rules out: VAPID keys in environment variables, the `web-push` crate, one job per channel for push. Native push
  (APNs/FCM directly) stays with the Tauri apps ([ADR-0009](0009-frontend-react-tauri.md)).
