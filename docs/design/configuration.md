# Design: Configuration

Status: **proposed** · All configuration via environment variables prefixed `HM_`. No config files in the MVP.

| Variable | Required | Default | Meaning |
|---|---|---|---|
| `HM_DATABASE_URL` | yes | — | Postgres connection string |
| `HM_MASTER_KEY` / `HM_MASTER_KEY_FILE` | yes (one of) | — | 32-byte base64 master key ([security](security.md)) |
| `HM_MASTER_KEYS` | no | — | Versioned keys for rotation (proposed format: `1:<b64>,2:<b64>`) |
| `HM_MODE` | no | `all` | `all` \| `api` \| `worker` |
| `HM_BIND` | no | `0.0.0.0:8080` | HTTP listen address |
| `HM_BASE_URL` | no | — | Public URL (links in notifications, cookies) |
| `HM_ADMIN_EMAIL` / `HM_ADMIN_PASSWORD` | no | — | Bootstrap admin on first start |
| `HM_SMTP_HOST` / `_PORT` / `_USER` / `_PASSWORD` / `_FROM` | for email | — | SMTP settings |
| `HM_VAPID_PUBLIC_KEY` / `HM_VAPID_PRIVATE_KEY` | for web push | — | Web Push keys |
| `HM_ALLOW_PRIVATE_TARGETS` | no | `true` self-hosted | Allow webhooks to private IP ranges |
| `HM_LOG` | no | `info` | `tracing` filter (`RUST_LOG` syntax) |

Rule: every new variable is added here **in the same PR** that introduces it.
