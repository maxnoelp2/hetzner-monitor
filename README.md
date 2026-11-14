# hetzner-monitor

Monitoring & alerting for **Hetzner Cloud** — paste a read-only API token, get meaningful alerts.
No agent on your servers, no Prometheus stack.

> **Status: 0.1.0** — the server and its HTTP API are usable and self-hostable. The web UI described in this guide is
> in development and ships with the next release; the 0.1.0 image has no UI yet, so with it follow the **With the API**
> instructions in each step (`curl` or any OpenAPI client). See the [changelog](CHANGELOG.md) and the
> [roadmap](docs/roadmap.md).

## Features

- Connect Hetzner Cloud projects with a **read-only** token (stored encrypted, never shown again)
- Auto-discovered inventory: servers, load balancers, volumes, certificates, floating and primary IPs
- Alerts for availability, CPU/network/disk I/O, **traffic quota with forecast**, certificate expiry, deprecations,
  backups/protection/orphaned resources and cost changes — default rules are created for you
- Rules with thresholds, durations and label selectors; silences; acknowledgements
- Notifications via email, Slack, Discord, signed webhooks, ntfy and **push notifications** on phones and desktops
  (Web Push), with retries
- A Hetzner API outage never produces "server down" alerts
- A web UI served by the same binary, installable on phones and desktops as an app (PWA); everything it does is also
  available through the HTTP API
- One Rust binary + PostgreSQL, multi-arch image for x86 and Arm (`linux/amd64`, `linux/arm64`)

Limits: the Hetzner API provides no RAM or disk-fill metrics — those would need an agent.

## Self-hosting

This guide takes you from an empty server to a first alert channel. You need about 15 minutes. After the start, all
setup happens in the web UI; each step also shows how to do it **with the API** instead.

### 1. Prerequisites

- A Linux host (x86 or Arm) with **Docker Engine 24+** and the **Compose plugin** (`docker compose version`).
- `curl` and `openssl` (preinstalled on most distributions), and a web browser.
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

### 5. Log in

Open **http://127.0.0.1:8080** in a browser and log in with the e-mail from `bootstrap` and the initial password. Then
change the password under **Settings → Account** (12+ characters).

The app listens on `127.0.0.1` only. On a remote server, either set up HTTPS first
([step 8](#8-https)) or open an SSH tunnel from your computer
(`ssh -L 8080:127.0.0.1:8080 you@your-server`) and use http://localhost:8080. If the login form comes back right after
you log in, the browser refused the `Secure` login cookie over plain HTTP — see [Troubleshooting](#troubleshooting).

**With the API.** The examples read secrets into shell variables, so they are neither echoed nor written to your shell
history. The admin API key works for every endpoint:

```bash
read -rs HM_API_KEY && export HM_API_KEY   # paste the key, press Enter
API=http://127.0.0.1:8080/api/v1
curl -fsS -H "Authorization: Bearer $HM_API_KEY" $API/me
```

Logging in with the password gives a session cookie instead, as the web UI does:

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

In the web UI, open **Projects** → **Connect project**, enter a name and paste the token. The token is stored
encrypted and never shown again. The first inventory sync runs right away, and the default rules are active
immediately (**Rules**); alerts appear under **Alerts**.

**With the API:**

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

For example Discord (Server settings → Integrations → Webhooks → copy the URL) or [ntfy](https://ntfy.sh).

In the web UI, open **Channels** → **Add channel**, choose the type, a name and the settings (for Discord the webhook
URL), then press **Send test** on the new channel. Channel settings are write-only: after saving, the UI shows only a
short hint, never the URL or secret.

Channels get every alert of `min_severity` (default `warning`) and above. Email channels need the `HM_SMTP_*`
settings. Push notifications to your phone use a **Web Push** channel ([step 9](#9-push-notifications-on-your-phone)).
All channel types and options: [docs/design/api.md](docs/design/api.md).

**With the API:**

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

### 8. HTTPS

Phones, the installable app and push notifications need HTTPS (browsers allow them only over HTTPS or on
`localhost`), and the login cookie is `Secure` unless `HM_BASE_URL` starts with `http://`. Pick one of the two ways
below, set `HM_BASE_URL` to the resulting `https://` URL in `.env` and apply it with `docker compose up -d`.

**A. Private, without a domain: Tailscale Serve.** If the host is in your [Tailscale](https://tailscale.com/) network,
Tailscale gives it an HTTPS name with a valid certificate that only your own devices can reach — no open port, no
domain, no proxy to run:

1. In the Tailscale admin console, under **DNS**, turn on MagicDNS and HTTPS certificates.
2. On the host: `sudo tailscale serve --bg 8080`. It prints the address, e.g. `https://monitor.tail1234.ts.net`, and
   keeps serving it after reboots (`tailscale serve status` shows it, `tailscale serve reset` removes it).
3. `HM_BASE_URL=https://monitor.tail1234.ts.net` in `.env`, then `docker compose up -d`.
4. Install the Tailscale app on your phone, log in to the same tailnet, and open that address.

**B. Public: a reverse proxy.** Put a reverse proxy with TLS in front of `127.0.0.1:8080`. With
[Caddy](https://caddyserver.com/), which gets certificates automatically, the whole `/etc/caddy/Caddyfile` is:

```
monitor.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

Notes:

- Always open the UI through `HM_BASE_URL` (the `https://` URL), not the server's IP. Over plain HTTP without
  `HM_BASE_URL=http://…`, browsers drop the login cookie.
- Login throttling counts failures per e-mail and per client address; behind a proxy all clients share the proxy's
  address, so the per-address limit (30 failures in 15 minutes) applies to all of them together.
- Compose binds the app and Postgres to `127.0.0.1` only; do not publish the Postgres port.
- Over HTTPS the browser offers to install the UI as an app ("Add to Home Screen" on iOS, "Install app" in Chrome and
  Edge). Notification links (`<HM_BASE_URL>/alerts/<id>`) open the alert in the UI. The app keeps no API data offline.

### 9. Push notifications on your phone

Web Push works with an `https://` `HM_BASE_URL` (step 8). Push services want a contact address; the base URL is used
unless you set `HM_VAPID_SUBJECT=mailto:you@example.com` in `.env`. The key pair the server signs with is created
automatically and stored encrypted, like the other secrets.

1. **Channels** → **Add channel** → type **Web Push**, a name (e.g. "Phones"). It sends to the devices of every member.
2. On the phone, open the `https://` address and log in.
   - **iPhone / iPad** (iOS 16.4 or later): first tap **Share** → **Add to Home Screen**, open hetzner-monitor from the
     home screen and log in there — Safari offers push only to apps on the home screen.
   - **Android**: Chrome works in the browser; installing the app is optional.
3. **Settings** → **Push notifications** → **Turn on push for this device**, allow notifications, then **Send test
   push**. Tapping a notification opens the alert.

Each phone or computer is a separate device in that list. **Remove** (or logging out on the device) stops
notifications to it; devices that uninstalled the app or revoked the permission are removed automatically on the next
notification. **Send test** on the Web Push channel reports the result per device.

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
| App exits: `HM_MASTER_KEY is invalid` | `HM_MASTER_KEY` missing or not 32 bytes base64 — regenerate with `openssl rand -base64 32` (only before the first secrets are stored). |
| `password authentication failed` in the logs | `.env` was created after the first start. Fresh install: `docker compose down -v` (**deletes all data**) and start again. |
| `bootstrap`: "already bootstrapped" | It only runs once; use `users reset-password` for a new password and **Settings → API keys** in the UI (or `POST /api-keys`) for more keys. |
| The login form comes back right after logging in (login works with curl but not in the browser) | The browser dropped the `Secure` login cookie on plain HTTP. Use HTTPS ([step 8](#8-https)), or for a test on plain HTTP set `HM_BASE_URL` to the `http://` address you open (e.g. `http://localhost:8080`) and run `docker compose up -d`. |
| The browser shows "The web UI is not built into this binary" | A binary built without the web UI (plain `cargo build`). Use the published image, or build with the UI as described under [Development](#development). |
| A second installation on the same host replaces the first one's containers | The Compose project is named `hetzner-monitor`. Run the other one with its own name and ports: `docker compose -p hm-staging up -d`, with different `HM_PORT` and `HM_POSTGRES_PORT` in its `.env`, and pass `-p hm-staging` to every later `docker compose` command. |
| Settings says "Push needs HTTPS" | The page was opened over plain HTTP (or by IP). Open the `https://` address from [step 8](#8-https). |
| Settings says "Web Push is not set up on this server" | `HM_BASE_URL` is `http://…` or unset and `HM_VAPID_SUBJECT` is not set. Set an `https://` `HM_BASE_URL` (or `HM_VAPID_SUBJECT=mailto:…`) and run `docker compose up -d`. |
| No "Turn on push" button on an iPhone | Add the app to the home screen first and open it from there (iOS 16.4 or later). |
| Project status `invalid_token` | Hetzner no longer accepts the token (deleted or expired). Tokens cannot be replaced yet: delete the project (**Delete project** on its page, or `DELETE /projects/{id}`; removes its alerts) and connect it again with a new read-only token. |

## API

Everything in the web UI uses the public HTTP API, so scripts can do the same. All endpoints are under `/api/v1`. The OpenAPI 3.1 description is served at `GET /api/v1/openapi.json` (also
`docker compose run --rm app openapi`); load it into any OpenAPI viewer or client generator. Authentication:
`Authorization: Bearer hm_…` or the session cookie (+ `X-CSRF-Token` for changes). Errors look like
`{"error": {"code": "not_found", "message": "…"}}`. Design notes: [docs/design/api.md](docs/design/api.md).

## Development

Building from source needs Rust (pinned in `rust-toolchain.toml`) and Docker for Postgres; the web UI needs Node.js 24
and npm.

```bash
docker compose -f deploy/docker-compose.yml up -d postgres
docker compose -f deploy/docker-compose.yml -f deploy/docker-compose.build.yml up --build -d   # app from this checkout

(cd web && npm ci && npm run build) && cargo build --features web-ui   # binary with the embedded UI
(cd web && npm run dev)   # UI dev server on :5173, proxies /api to :8080 (keeps running; use a second terminal)
```

A plain `cargo build` works without Node; that binary serves a placeholder page instead of the UI. Details:
[docs/design/web.md](docs/design/web.md).

Commands, test setup and conventions: [CONTRIBUTING.md](CONTRIBUTING.md) and [CLAUDE.md](CLAUDE.md).

## Documentation

- [Architecture](ARCHITECTURE.md)
- [Product scope](docs/product.md) · [Roadmap](docs/roadmap.md) · [Decisions (ADRs)](docs/adr/README.md)
- [All docs](docs/README.md) · [Security policy](SECURITY.md)

## License

[AGPL-3.0](LICENSE)
