# Design: Configuration

Status: **partly implemented** (column *Since*) · All configuration via environment variables prefixed `HM_`. No config files in the MVP.
Source: `crates/server/src/config.rs`.

| Variable | Required | Default | Since | Meaning |
|---|---|---|---|---|
| `HM_DATABASE_URL` | yes | — | M0 | Postgres connection string |
| `HM_MASTER_KEY` / `HM_MASTER_KEY_FILE` | yes (one of), from M1 | — | planned (M1) | 32-byte base64 master key ([security](security.md)) |
| `HM_MASTER_KEYS` | no | — | planned | Versioned keys for rotation (proposed format: `1:<b64>,2:<b64>`) |
| `HM_MODE` | no | `all` | M0 (parsed, logged) | `all` \| `api` \| `worker` |
| `HM_BIND` | no | `0.0.0.0:8080` | M0 | HTTP listen address |
| `HM_BASE_URL` | no | — | planned | Public URL (links in notifications, cookies) |
| `HM_ADMIN_EMAIL` / `HM_ADMIN_PASSWORD` | no | — | planned | Bootstrap admin on first start |
| `HM_SMTP_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_FROM` | for email | — | planned | SMTP settings |
| `HM_VAPID_PUBLIC_KEY` / `HM_VAPID_PRIVATE_KEY` | for web push | — | planned | Web Push keys |
| `HM_ALLOW_PRIVATE_TARGETS` | no | `true` self-hosted | planned | Allow webhooks to private IP ranges |
| `HM_LOG` | no | `info` | M0 | `tracing` filter (`RUST_LOG` syntax) |

Docker Compose additionally reads `HM_POSTGRES_PASSWORD`, `HM_POSTGRES_PORT` (default `55432`) and `HM_PORT` (default `8080`) — see `deploy/.env.example`. Integration tests use `DATABASE_URL` (sqlx convention).

Rule: every new variable is added here **in the same PR** that introduces it.
