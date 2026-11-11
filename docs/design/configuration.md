# Design: Configuration

Status: **partly implemented** (column *Since*) · All configuration via environment variables prefixed `HM_`. No config files in the MVP.
Source: `crates/server/src/config.rs`.

| Variable | Required | Default | Since | Meaning |
|---|---|---|---|---|
| `HM_DATABASE_URL` | yes | — | M0 | Postgres connection string |
| `HM_MASTER_KEY` / `HM_MASTER_KEY_FILE` | yes (exactly one) | — | M1 | 32-byte base64 master key ([security](security.md)) |
| `HM_MASTER_KEYS` | no | — | planned | Versioned keys for rotation (proposed format: `1:<b64>,2:<b64>`) |
| `HM_MODE` | no | `all` | M0 (parsed, logged) | `all` \| `api` \| `worker` |
| `HM_BIND` | no | `0.0.0.0:8080` | M0 | HTTP listen address |
| `HM_BASE_URL` | no | — | planned | Public URL (links in notifications, cookies) |
| `HM_SMTP_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_FROM` | for email | — | planned | SMTP settings |
| `HM_VAPID_PUBLIC_KEY` / `HM_VAPID_PRIVATE_KEY` | for web push | — | planned | Web Push keys |
| `HM_ALLOW_PRIVATE_TARGETS` | no | `true` self-hosted | planned | Allow webhooks to private IP ranges |
| `HM_LOG` | no | `info` | M0 | `tracing` filter (`RUST_LOG` syntax) |

The first organisation, owner and admin API key are created with the CLI, not env vars:
`hetzner-monitor bootstrap --org <name> --email <email>` (empty database only; prints the key once).
Generate a master key with `hetzner-monitor master-key generate` (or `openssl rand -base64 32`).

Docker Compose additionally reads `HM_POSTGRES_PASSWORD`, `HM_POSTGRES_PORT` (default `55432`) and `HM_PORT` (default `8080`) — see `deploy/.env.example`. Postgres applies `HM_POSTGRES_PASSWORD` only when its volume is first initialised: create `deploy/.env` **before** the first `up`, otherwise changing the password later needs `docker compose -f deploy/docker-compose.yml down -v` (deletes all data) or an `ALTER USER` inside Postgres. Integration tests use `DATABASE_URL` (sqlx convention).

Rule: every new variable is added here **in the same PR** that introduces it.
