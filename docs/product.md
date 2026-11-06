# Product

## Vision

"Paste a Hetzner token, get meaningful alerts in two minutes — no Prometheus stack, no agent."

Generic monitoring tools don't know Hetzner specifics (traffic quota, deprecations, managed
certificates, load balancer target health, cost). The Prometheus exporter route needs a whole
stack. hetzner-monitor fills that gap.

## Target users

Anyone running servers on Hetzner Cloud: solo developers, agencies managing customer projects,
small ops teams. Usable self-hosted (Docker Compose) and later as a hosted SaaS.

## Distribution

- **Open core**, AGPL-3.0, **all features open source** ([ADR-0008](adr/0008-agpl-and-open-core.md)).
  The hosted SaaS earns through hosting, convenience and support — not through locked features.
- Surfaces in order: headless core + API → React PWA → Tauri 2 mobile → Tauri 2 desktop
  ([ADR-0009](adr/0009-frontend-react-tauri.md)).

## MVP scope

Data source: **Hetzner Cloud API only**, connected via a read-only token ([ADR-0001](adr/0001-cloud-api-only-no-agent.md)).

1. **Connect** one or more Hetzner projects (one token per project), validate token, store encrypted.
2. **Inventory** auto-discovery: servers, load balancers, volumes, certificates, floating/primary IPs; labels; created/deleted diff.
3. **Checks** — all four groups ([details](design/checks.md)):
   - Availability: server status, resource created/deleted, LB target health
   - Utilisation: CPU (normalised per vCPU), network, disk I/O over threshold for a duration
   - Hetzner-specific: traffic quota + forecast, certificate expiry/renewal failure, deprecations
   - Hygiene & cost: backups off, protection off, orphaned volumes/IPs, monthly cost change
4. **Rules**: threshold + duration + severity + scope (project, resource, Hetzner label selector); sensible defaults on connect.
5. **Alert lifecycle**: pending → firing → resolved, dedup, re-notify, acknowledge, silences/maintenance windows, history.
6. **Notifications**: email, Slack, Discord, generic webhook (HMAC-signed), ntfy, Web Push. *(Telegram: not in MVP.)*
7. **Multi-tenant** from day one: organisation → users → projects ([ADR-0006](adr/0006-multi-tenant-from-day-one.md)).
8. **Auth**: server-side sessions for the PWA, scoped API keys for headless use ([ADR-0007](adr/0007-auth-sessions-and-api-keys.md)).

## Known limits (communicate clearly to users)

Without an agent the Hetzner API cannot provide: **RAM usage, disk fill level** (only IOPS/bandwidth),
processes/services, logs, private-network traffic. See [research](research/hetzner-cloud-api.md#not-available).

## Non-goals (MVP)

- Agent on servers, external HTTP/ping/SSL checks, dashboards/graphs, Hetzner Robot (dedicated) and
  Storage Box, Telegram, SMS/PagerDuty, TOTP/OIDC, billing. All are candidates for later phases.
- Writing to Hetzner in any form (start/stop/reboot). The tool is read-only by design.

## Open questions

- Product name: "Hetzner" is a trademark — check before a public launch.
- Default thresholds for network and disk I/O, or templates only?
