# hetzner-monitor

Monitoring & alerting for **Hetzner Cloud** — paste a read-only API token, get meaningful alerts.
No agent on your servers, no Prometheus stack.

> **Status: 0.1.0** — the server and its HTTP API are usable and self-hostable. There is no web UI yet (the PWA comes
> in a later release); you work with the API via `curl` or any OpenAPI client. See the [changelog](CHANGELOG.md) and
> the [roadmap](docs/roadmap.md).

## Features

- Connect Hetzner Cloud projects with a **read-only** token (stored encrypted, never shown again)
- Auto-discovered inventory: servers, load balancers, volumes, certificates, floating and primary IPs
- Alerts for availability, CPU/network/disk I/O, **traffic quota with forecast**, certificate expiry, deprecations,
  backups/protection/orphaned resources and cost changes — default rules are created for you
- Rules with thresholds, durations and label selectors; silences; acknowledgements
- Notifications via email, Slack, Discord, signed webhooks and ntfy, with retries
- A Hetzner API outage never produces "server down" alerts
- One Rust binary + PostgreSQL, multi-arch image for x86 and Arm (`linux/amd64`, `linux/arm64`)

Limits: the Hetzner API provides no RAM or disk-fill metrics — those would need an agent.

## Self-hosting

This guide takes you from an empty server to a first alert channel. You need about 15 minutes.

### 1. Prerequisites

- A Linux host (x86 or Arm) with **Docker Engine 24+** and the **Compose plugin** (`docker compose version`).
- `curl` and `openssl` (preinstalled on most distributions).
- For a public setup: a domain name and a reverse proxy for HTTPS (example below). The app itself listens only on
  `127.0.0.1`.

### 2. Get the Compose file and create `.env`

```bash
mkdir -p ~/hetzner-monitor && cd ~/hetzner-monitor
curl -fsSLO https://raw.githubusercontent.com/maxnoelp2/hetzner-monitor/v0.1.0/deploy/docker-compose.yml
```

Create `.env` next to it with a generated master key and database password. Do this **before the first start** —
Postgres takes its password only when its volume is created.

```bash
umask 077
cat > .env <<EOF
HM_VERSION=0.1.0
HM_MASTER_KEY=$(openssl rand -base64 32)
HM_POSTGRES_PASSWORD=$(openssl rand -hex 24)
# Public URL (links in notifications; http:// turns off the Secure flag of the login cookie):
HM_BASE_URL=https://monitor.example.com
EOF
```

> **Back up the master key now** (password manager or another safe place). It encrypts the stored Hetzner tokens and
> channel settings; without it a database backup is useless. Never commit `.env` or paste it anywhere.

Optional settings (SMTP for email channels, log level, ports, …) are listed in
[docs/design/configuration.md](docs/design/configuration.md); add them to `.env`. For a server that is reachable from
the internet, `HM_ALLOW_PRIVATE_TARGETS=false` prevents webhooks to internal addresses.

### 3. Start

```bash
docker compose up -d
docker compose ps                      # app becomes "healthy" after a few seconds
curl -fsS http://127.0.0.1:8080/readyz # {"status":"ok"}
```

Database migrations run automatically on startup. `docker compose logs -f app` shows the logs.

### 4. Create the organisation and the first user

```bash
docker compose run --rm app bootstrap --org "My company" --email you@example.com
```

This works once, on an empty database. It prints an **admin API key** (`hm_…`) and the owner's **initial password**,
each shown only once — store both in your password manager. Lost the password later?
`docker compose run --rm app users reset-password --email you@example.com` prints a new one.

The examples below read secrets into shell variables, so they are neither echoed nor written to your shell history:

```bash
read -rs HM_API_KEY && export HM_API_KEY   # paste the key, press Enter
API=http://127.0.0.1:8080/api/v1
curl -fsS -H "Authorization: Bearer $HM_API_KEY" $API/me
```

### 5. Log in with the password (optional)

Browsers and the coming web app use the password and a session cookie instead of the API key:

```bash
read -rs HM_PASSWORD && export HM_PASSWORD
printf '{"email":"you@example.com","password":"%s"}' "$HM_PASSWORD" |
  curl -fsS -c cookies.txt -H 'Content-Type: application/json' --data @- $API/auth/login
```

The response contains a `csrf_token`; send it as `X-CSRF-Token` with every cookie request that changes something.
Change the initial password with `PUT /me/password` (`{"current_password": …, "new_password": …}`, 12+ characters).
Delete `cookies.txt` when you are done.

### 6. Connect a Hetzner Cloud project

In the [Hetzner Cloud Console](https://console.hetzner.cloud/): open the project → **Security** → **API tokens** →
**Generate API token**, permission **Read**. hetzner-monitor only ever reads — a read-only token is all it needs and
limits the damage if the token ever leaks.

```bash
read -rs HCLOUD_TOKEN && export HCLOUD_TOKEN
printf '{"name":"production","token":"%s"}' "$HCLOUD_TOKEN" |
  curl -fsS -H "Authorization: Bearer $HM_API_KEY" -H 'Content-Type: application/json' --data @- $API/projects
unset HCLOUD_TOKEN
```

The response shows the project and the first inventory sync. Default rules are active right away:

```bash
curl -fsS -H "Authorization: Bearer $HM_API_KEY" $API/rules
curl -fsS -H "Authorization: Bearer $HM_API_KEY" "$API/alerts?state=open"
```

### 7. Add a notification channel

For example Discord (Server settings → Integrations → Webhooks → copy the URL) or [ntfy](https://ntfy.sh):

```bash
read -rs WEBHOOK_URL
printf '{"type":"discord","name":"ops","config":{"url":"%s"}}' "$WEBHOOK_URL" |
  curl -fsS -H "Authorization: Bearer $HM_API_KEY" -H 'Content-Type: application/json' --data @- $API/channels
# ntfy instead: --data '{"type":"ntfy","name":"phone","config":{"topic":"<a long random topic>"}}'
unset WEBHOOK_URL
```

Send a test message with the channel `id` from the response:

```bash
curl -fsS -X POST -H "Authorization: Bearer $HM_API_KEY" $API/channels/<id>/test   # {"ok":true}
```

Channels get every alert of `min_severity` (default `warning`) and above. Email channels need the `HM_SMTP_*`
settings. All channel types and options: [docs/design/api.md](docs/design/api.md).

### 8. HTTPS with a reverse proxy

Put a reverse proxy with TLS in front of `127.0.0.1:8080` and set `HM_BASE_URL` to the public `https://` URL
(then `docker compose up -d` to apply). With [Caddy](https://caddyserver.com/), which gets certificates
automatically, the whole `/etc/caddy/Caddyfile` is:

```
monitor.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Notes:

- The login cookie is `Secure` unless `HM_BASE_URL` starts with `http://` — so over plain HTTP without that setting,
  browsers drop it. Use HTTPS for anything beyond `localhost`.
- Login throttling counts failures per e-mail and per client address; behind a proxy all clients share the proxy's
  address, so the per-address limit (30 failures in 15 minutes) applies to all of them together.
- Compose binds the app and Postgres to `127.0.0.1` only; do not publish the Postgres port.

### Upgrading

Read the [changelog](CHANGELOG.md) for the target version (before 1.0, minor releases may change the API or
settings). Back up first, then:

```bash
sed -i 's/^HM_VERSION=.*/HM_VERSION=0.1.1/' .env   # the version you upgrade to
curl -fsSLO https://raw.githubusercontent.com/maxnoelp2/hetzner-monitor/v0.1.1/deploy/docker-compose.yml
docker compose pull && docker compose up -d
```

Migrations run on startup and cannot be rolled back; to go back, restore the backup with the old version.

### Backups

Two things make a complete backup — the database **and** the master key (`.env`):

```bash
docker compose exec -T postgres pg_dump -U hetzner_monitor -Fc hetzner_monitor > hm-$(date +%F).dump
```

Restore into a fresh installation with the same `.env`:

```bash
docker compose up -d postgres
docker compose exec -T postgres pg_restore -U hetzner_monitor -d hetzner_monitor --clean --if-exists < hm-2026-11-12.dump
docker compose up -d
```

### Rotating the master key

Add a new key next to the old one (`HM_MASTER_KEYS=1:<old>,2:<new>` instead of `HM_MASTER_KEY`), restart, and run
`docker compose run --rm app keys rotate`. The full runbook is in
[docs/design/security.md](docs/design/security.md#envelope-encryption-for-secrets-at-rest).

### Troubleshooting

| Symptom | Cause / fix |
|---|---|
| App exits with "master key" error | `HM_MASTER_KEY` missing or not 32 bytes base64 — regenerate with `openssl rand -base64 32` (only before the first secrets are stored). |
| `password authentication failed` in the logs | `.env` was created after the first start. Fresh install: `docker compose down -v` (**deletes all data**) and start again. |
| `bootstrap` says the database is not empty | It only runs once; use `users reset-password` for a new password. |
| Login works with curl but not in the browser | HTTP without `HM_BASE_URL=http://…` — use HTTPS (reverse proxy) instead. |
| Project status `invalid_token` | Hetzner no longer accepts the token (deleted or expired). Tokens cannot be replaced yet: delete the project (`DELETE /projects/{id}`, removes its alerts) and connect it again with a new read-only token. |

## API

All endpoints are under `/api/v1`. The OpenAPI 3.1 description is served at `GET /api/v1/openapi.json` (also
`docker compose run --rm app openapi`); load it into any OpenAPI viewer or client generator. Authentication:
`Authorization: Bearer hm_…` or the session cookie (+ `X-CSRF-Token` for changes). Errors look like
`{"error": {"code": "not_found", "message": "…"}}`. Design notes: [docs/design/api.md](docs/design/api.md).

## Development

Building from source needs Rust (pinned in `rust-toolchain.toml`) and Docker for Postgres.

```bash
docker compose -f deploy/docker-compose.yml up -d postgres
docker compose -f deploy/docker-compose.yml -f deploy/docker-compose.build.yml up --build -d   # app from this checkout
```

Commands, test setup and conventions: [CONTRIBUTING.md](CONTRIBUTING.md) and [CLAUDE.md](CLAUDE.md).

## Documentation

- [Architecture](ARCHITECTURE.md)
- [Product scope](docs/product.md) · [Roadmap](docs/roadmap.md) · [Decisions (ADRs)](docs/adr/README.md)
- [All docs](docs/README.md) · [Security policy](SECURITY.md)

## License

[AGPL-3.0](LICENSE)
