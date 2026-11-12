# Design: Configuration

Status: **partly implemented** (column *Since*) · All configuration via environment variables prefixed `HM_`. No config files in the MVP.
Source: `crates/server/src/config.rs`.

| Variable | Required | Default | Since | Meaning |
|---|---|---|---|---|
| `HM_DATABASE_URL` | yes | — | M0 | Postgres connection string |
| `HM_MASTER_KEY` / `HM_MASTER_KEY_FILE` | yes (exactly one) | — | M1 | 32-byte base64 master key ([security](security.md)) |
| `HM_MASTER_KEYS` | no | — | planned | Versioned keys for rotation (proposed format: `1:<b64>,2:<b64>`) |
| `HM_MODE` | no | `all` | M2 | `all` (API + scheduler) \| `api` (no scheduler) \| `worker` (scheduler; still serves `/healthz`, `/readyz`, API) |
| `HM_BIND` | no | `0.0.0.0:8080` | M0 | HTTP listen address |
| `HM_BASE_URL` | no | — | M4 | Public URL, `http(s)://…`; links in notifications (`<base>/api/v1/alerts/{id}`), later cookies |
| `HM_SMTP_HOST` | for email | — | M4 | SMTP server; unset = email channels unavailable (`422 email_disabled`) |
| `HM_SMTP_FROM` | with `HM_SMTP_HOST` | — | M4 | Sender address, e.g. `hetzner-monitor <hm@example.com>` |
| `HM_SMTP_TLS` | no | `starttls` | M4 | `starttls` (required) \| `tls` (implicit) \| `none` (local relays only) |
| `HM_SMTP_PORT` | no | 587 / 465 / 25 | M4 | Default by `HM_SMTP_TLS` |
| `HM_SMTP_USER` / `HM_SMTP_PASSWORD` | no (both or neither) | — | M4 | SMTP login; the password is never logged |
| `HM_VAPID_PUBLIC_KEY` / `HM_VAPID_PRIVATE_KEY` | for web push | — | planned | Web Push keys |
| `HM_ALLOW_PRIVATE_TARGETS` | no | `true` | M4 | `false` refuses Slack/Discord/webhook/ntfy targets that resolve to private, loopback, link-local, CGNAT or metadata addresses ([security](security.md#outbound-requests-ssrf)) |
| `HM_LOG` | no | `info` | M0 | `tracing` filter (`RUST_LOG` syntax) |

The first organisation, owner and admin API key are created with the CLI, not env vars:
`hetzner-monitor bootstrap --org <name> --email <email>` (empty database only; prints the key once).
Generate a master key with `hetzner-monitor master-key generate` (or `openssl rand -base64 32`).

Docker Compose additionally reads `HM_POSTGRES_PASSWORD`, `HM_POSTGRES_PORT` (default `55432`) and `HM_PORT` (default `8080`) — see `deploy/.env.example`. Postgres applies `HM_POSTGRES_PASSWORD` only when its volume is first initialised: create `deploy/.env` **before** the first `up`, otherwise changing the password later needs `docker compose -f deploy/docker-compose.yml down -v` (deletes all data) or an `ALTER USER` inside Postgres. Integration tests use `DATABASE_URL` (sqlx convention).

Rule: every new variable is added here **in the same PR** that introduces it.
