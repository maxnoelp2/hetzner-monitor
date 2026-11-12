# Design: Checks

Status: **accepted** (scope); `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota`, `hetzner_api` **implemented** (M2), `cpu`, `network`, `disk_io` **implemented** (M3a), `certificate_expiry`, `deprecation`, `hygiene_*`, `cost_change` and the traffic forecast **implemented** (M3b) · Crate: `crates/core` (`checks`, `cost`)

A check is a Rust type that takes an inventory snapshot and/or a metric series and returns a
value plus a boolean condition. Checks are pure: no IO, unit-tested with fixtures.

| `check_type` | Tier | Default condition | Default severity |
|---|---|---|---|
| `server_status` | fast | status ≠ `running` for ≥ 2 cycles; ignore `starting`/`migrating`/`rebuilding` for < 10 min | critical |
| `resource_lifecycle` | fast / inventory | resource created or deleted (inventory diff) | info |
| `lb_target_health` | fast | any target service `health_status` ≠ `healthy` | critical |
| `cpu` | metrics | `cpu / server_type.cores` > 90 % for 10 min | warning |
| `network` | metrics | bandwidth or pps > threshold for 5 min — **template only, no default threshold** | warning |
| `disk_io` | metrics | IOPS or bandwidth > threshold for 10 min — **template only** | warning |
| `traffic_quota` | fast (forecast: inventory) | `outgoing_traffic / included_traffic` ≥ 80 % (warning) / 100 % (critical), or forecast exceeds quota before the end of the calendar month | warning / critical |
| `certificate_expiry` | inventory | `not_valid_after` < 14 days (warning) / < 3 days (critical), or issuance/renewal `failed` | warning / critical |
| `deprecation` | inventory | server type deprecated in the server's location, or load balancer type deprecated | info |
| `hygiene_backups` | inventory | `backup_window` is null for 1 h | info |
| `hygiene_protection` | inventory | delete (servers: or rebuild) protection off for 1 h | info |
| `hygiene_orphans` | inventory | volume without `server`; floating / primary IP unassigned, for 1 day | info |
| `cost_change` | inventory | estimated monthly cost (`/pricing` × inventory) ≥ +10 % vs. the latest earlier day | info |
| `hetzner_api` | every cycle | project status ≠ `active` (system alert, [invariant 6](alerting.md#data-source-failure-invariant-6)) | critical |

## Default rules and params (M2)

Created per project on connect. Params are validated on `PATCH /rules/{id}`; unknown fields are rejected.

| Rule | `check_type` | Params (defaults) | Severity |
|---|---|---|---|
| Server not running | `server_status` | `for_s` 120, `transitional_grace_s` 600 | critical |
| Resource created or deleted | `resource_lifecycle` | — | info |
| Load balancer target unhealthy | `lb_target_health` | `for_s` 120 | critical |
| Traffic quota 80 % used | `traffic_quota` | `threshold_pct` 80, `for_s` 0 | warning |
| Traffic quota exhausted | `traffic_quota` | `threshold_pct` 100, `for_s` 0 | critical |
| Hetzner API unreachable | `hetzner_api` | — | critical |
| CPU high | `cpu` | `threshold_pct` 90, `for_s` 600 | warning |
| Certificate expiring or renewal failed | `certificate_expiry` | `days` 14, `on_failure` true | warning |
| Certificate expires within 3 days | `certificate_expiry` | `days` 3, `on_failure` false | critical |
| Type deprecated | `deprecation` | — | info |
| Server backups disabled | `hygiene_backups` | `for_s` 3600 | info |
| Delete/rebuild protection off | `hygiene_protection` | `kinds` `["server"]`, `for_s` 3600 | info |
| Unattached volume or IP | `hygiene_orphans` | `for_s` 86400 | info |
| Estimated cost increased | `cost_change` | `threshold_pct` 10 | info |
| Traffic quota forecast exceeded | `traffic_quota` | `threshold_pct` 100, `forecast` true | warning |

Templates without a default rule (create with `POST /rules`; `metric` and `threshold` are required):

| `check_type` | `metric` | `threshold` unit | `for_s` default |
|---|---|---|---|
| `network` | `bandwidth_in`, `bandwidth_out` · `pps_in`, `pps_out` | bytes/s · packets/s (public interface) | 300 |
| `disk_io` | `iops_read`, `iops_write` · `bandwidth_read`, `bandwidth_write` | IOPS · bytes/s | 300 |

- Metric checks (`cpu`, `network`, `disk_io`) run on the metrics tier and measure their duration on Hetzner's series
  ([scheduler](scheduler.md#metric-query-window)); value = newest sample (CPU in % of the whole server, one decimal).
- `cpu`: `threshold_pct` is a share of the **whole server** (0–100): the series value is divided by `server_type.cores`
  from the server snapshot (no extra request; missing → 1).

- `server_status`: "≥ 2 cycles" is expressed as `for_s = 120`. Transitional statuses (`initializing`, `starting`,
  `migrating`, `rebuilding`, `deleting`) use `max(for_s, transitional_grace_s)` instead.
- `lb_target_health`: counts every target service (`targets[].health_status[]`, and the nested `targets[]` of
  label-selector targets); `unknown` counts as not healthy, which `for_s` absorbs for new targets. Value = number of bad services.
- `traffic_quota`: servers and load balancers; value = percent used (one decimal). `null` traffic fields → no data, condition false.
  Two default rules instead of one with escalating severity: at ≥ 100 % both are open.
- Field shapes verified against the OpenAPI spec on 2026-11-11 (M3b fields on 2026-11-12).

## Inventory checks (M3b)

Evaluated on **inventory cycles only** (connect, manual sync, every 15 min), over all six resource kinds. Fast cycles
skip these rules entirely, so their alerts keep their state. Hold times (`for_s`) use the alert's local pending time.

- `certificate_expiry` (certificates): fires when `not_valid_after` is less than `days` (1–365) away or past, or with
  `on_failure` when `status.issuance` or `status.renewal` is `failed` (message carries `status.error.message`).
  Uploaded certificates have no `status` and are checked on expiry only; `not_valid_after` null → no data. Value = whole days left.
- `deprecation` (servers, load balancers): the server type's entry in `server_type.locations[]` for `server.location.name`
  has a `deprecation` (fallback: the deprecated top-level `server_type.deprecation`); for load balancers
  `load_balancer_type.deprecation`. Value = days until `unavailable_after`. Image deprecation is not checked
  (it affects rebuilds, not running servers).
- `hygiene_backups` (servers): `backup_window` null. `hygiene_protection`: `protection.delete` off, for servers also
  `protection.rebuild`; `kinds` ⊆ server, load_balancer, volume, floating_ip, primary_ip (default `["server"]`).
  `hygiene_orphans`: volume without `server` (not while `creating`), floating IP without `server`, primary IP without `assignee_id`.
  All hygiene checks are `info`; to exempt resources, set the rule's label selector (e.g. `backups!=off`) or disable it.
- `cost_change` (project-level, one alert per rule): today's [cost estimate](#cost-estimate) is ≥ `threshold_pct` above
  the latest estimate of an earlier UTC day. No earlier day, or a currency change → no data. Value = change in percent.
  The alert resolves once the new level is the earlier day's estimate (the next day) and two inventory cycles agree.
- `traffic_quota` with `forecast: true` (servers, load balancers): `outgoing + rate × (end of calendar month UTC − now)`
  ≥ `threshold_pct` of `included_traffic`, where `rate` is measured against the newest stored daily sample at least
  24 h old ([ADR-0011](../adr/0011-derived-daily-snapshots.md)). Counter below the sample (reset) or no sample → no data.
  Value = forecast percent. Every message says it is an estimate assuming a reset on the 1st (UTC).

### Cost estimate

`hm_core::cost::estimate` — net monthly prices from `GET /pricing` (one request per inventory cycle):
server type by location (+ `server_backup.percentage` when `backup_window` is set; fallback: the `prices` embedded in
the snapshot), load balancer type by location, volume `size` × `price_per_gb_month`, primary IP by type + `location`,
floating IP by type + `home_location`. Hourly billing is capped at the monthly price, so this is the full-month figure.
**Not included:** traffic overage, snapshots/images, object storage. Resources without a price are counted (`unpriced`)
and not guessed. Stored per project and UTC day in `cost_estimates`; the latest one is shown on `GET /projects/{id}`.

## Notes and assumptions

- **CPU (verified 2026-11-11 in the spec text):** Hetzner reports per vCPU — 100 = one full core; 400 = full load on 4 vCPUs ("divide the metric value by the number of vCPUs"). Always divide by `cores`.
- **Traffic forecast (assumption):** the billing period is not in the API, and the billing FAQ (2026-11-11) does not state the counter
  reset date. Assume calendar month (UTC), linear extrapolation from a day-old sample; labelled as an estimate in every message.
- **`server_type.deprecation` is deprecated** in the spec — read `server_type.locations[].deprecation` instead.
- **Not possible without an agent:** RAM, disk fill level, processes, logs. Do not fake these.
