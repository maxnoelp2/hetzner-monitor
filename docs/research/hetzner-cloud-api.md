# Research: Hetzner Cloud API for monitoring

As of **2026-11-05**. Primary source: [OpenAPI spec](https://docs.hetzner.cloud/cloud.spec.json) (all API facts below are **from spec** unless marked otherwise). Re-verify before relying on a number.

## Authentication

- `Authorization: Bearer <token>`. **A token is scoped to exactly one project** → one connection per project.
- Token permissions: *Read* (GET only) or *Read & Write*. Read is sufficient and recommended. ([token docs](https://docs.hetzner.com/cloud/api/getting-started/generating-api-token))

## Rate limit

- **3600 requests/hour per project**, refilling 1 per second, bursts allowed.
- Headers: `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset` (UNIX ts). Exceeded → `429`.
- Shared with all other users of the project (Terraform, CI, CLI).

| Metrics interval | Requests/h for N servers | Max servers at 50 % budget |
|---|---|---|
| 1 min | (N+1) × 60 | ~29 |
| 5 min | (N+1) × 12 | ~149 |
| 15 min | (N+1) × 4 | ~449 |

(Naive single-tier model; the tiered design in [scheduler](../design/scheduler.md) improves on this.)

## Pagination

`page`, `per_page` (default 25, **max 50**). Use `meta.pagination` in the body. The `Link` header is deprecated (changelog 2026-10-12).

## Server metrics — `GET /servers/{id}/metrics`

Params: `type` (`cpu`, `disk`, `network`, comma-combinable), `start`, `end`, `step`.

| Type | Series | Unit |
|---|---|---|
| cpu | `cpu` | percent **per vCPU** (100 = one core; stated explicitly in the spec, re-checked 2026-11-11) |
| disk | `disk.0.iops.read/write`, `disk.0.bandwidth.read/write` | iop/s, bytes/s |
| network | `network.0.pps.in/out`, `network.0.bandwidth.in/out` | packets/s, bytes/s (public interface only) |

- Retention **30 days**. Max **500 samples** per response; without `step` ≈ 200 samples.
- Values are strings: `[1435781470.622, "42"]`.

## Load balancer metrics — `GET /load_balancers/{id}/metrics`

Types: `open_connections`, `connections_per_second`, `requests_per_second`, `bandwidth`.
Each LB target carries `health_status` per service.

## Monitoring-relevant fields

| Resource | Field | Use |
|---|---|---|
| Server | `status`: running, initializing, starting, stopping, off, deleting, migrating, rebuilding, unknown | status alerts |
| Server | `locked` | running action |
| Server | `outgoing_traffic`, `ingoing_traffic`, `included_traffic` (bytes, current billing period) | traffic quota |
| Server | `backup_window` (null = backups off), `protection`, `labels` | hygiene, scoping |
| Server type | `deprecation` (**deprecated field**) → use `locations[].deprecation.unavailable_after` matched by `server.location.name` (embedded in every server object; locations themselves carry no deprecation, 2026-11-12) | deprecation alert |
| Load balancer type | `load_balancer_type.deprecation`, `prices[]` (embedded) | deprecation, cost |
| Load balancer | traffic fields, `targets[].health_status` | quota, health |
| Certificate | `type` uploaded/managed, `not_valid_after` (nullable), `status` (null for uploaded): `issuance` pending/completed/failed, `renewal` scheduled/pending/failed/unavailable, `error` | expiry / renewal |
| Volume | `status` (available, creating), `server` (nullable), `size` (GB), `protection.delete` | orphans, cost |
| Floating IP | `type`, `server` (nullable), `home_location.name`, `protection.delete` | orphans, cost |
| Primary IP | `type`, `assignee_id` (nullable), `assignee_type`, `location.name`, `protection.delete` | orphans, cost |
| `/pricing` (not paginated) | `currency`, `vat_rate`, `server_types[]`/`load_balancer_types[]`.`prices[]` per location, `volume.price_per_gb_month`, `primary_ips[]`/`floating_ips[]` per type + location, `server_backup.percentage`; prices are decimal strings `{net, gross}` (verified 2026-11-12) | cost estimate |

Project-wide action listing (`GET /actions` without IDs) was **removed on 2025-01-30** → detect changes by inventory diff.

## Not available

Without an agent: **RAM usage, disk fill level**, processes/services, logs, private-network traffic.

## Existing solutions (web research, confidence: medium)

| Solution | Type | Gap |
|---|---|---|
| Hetzner Cloud Console | graphs per server | Built-in threshold alerting for Cloud servers **not confirmed** — verify. Robot (dedicated) has threshold monitoring. |
| [promhippie/hcloud_exporter](https://promhippie.github.io/hcloud_exporter) | Prometheus exporter, Apache-2.0 | needs Prometheus + Alertmanager + Grafana; last release ~2 years old per [ecosyste.ms](https://awesome.ecosyste.ms/projects/github.com%2Fpromhippie%2Fhcloud_exporter) |
| [Netdata](https://www.netdata.cloud/monitoring/hetzner) etc. | agent | not Hetzner-aware (quota, certs, deprecations) |
| [TigerOps](https://tigerops-site.pages.dev/integrations/hetzner-cloud) | commercial SaaS | paid, generic focus (vendor page) |
| StatusGator, IncidentHub | Hetzner status pages | monitor Hetzner itself, not your servers |

## Traffic billing (Hetzner docs, 2026-11-11)

From the [billing FAQ](https://docs.hetzner.com/cloud/billing/faq/): only **outgoing** traffic is billed (incoming and
internal traffic are free); overage is billed in 100 MB blocks; invoices cover full calendar months after the first one;
the project owner gets Hetzner's own notice at 75 % and 100 % of the included traffic. **Not stated:** when the
`outgoing_traffic` counter resets — the calendar-month assumption stays an assumption.
