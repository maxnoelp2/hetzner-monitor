# Design: Checks

Status: **accepted** (scope); `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota`, `hetzner_api` **implemented** (M2), the rest **proposed** · Crate: `crates/core` (`checks`)

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
| `traffic_quota` | fast | `outgoing_traffic / included_traffic` ≥ 80 % (warning) / 100 % (critical), or forecast exceeds quota before period end | warning / critical |
| `certificate_expiry` | inventory | `not_valid_after` < 14 days, or `status.renewal` = `failed` | warning / critical |
| `deprecation` | inventory | `unavailable_after` set on server type / location | info |
| `hygiene_backups` | inventory | `backup_window` is null | info |
| `hygiene_protection` | inventory | delete/rebuild protection off | info |
| `hygiene_orphans` | inventory | volume without `server`; IP unassigned | info |
| `cost_change` | inventory | estimated monthly cost (`/pricing` × inventory) +X % vs. previous day | info |
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

- `server_status`: "≥ 2 cycles" is expressed as `for_s = 120`. Transitional statuses (`initializing`, `starting`,
  `migrating`, `rebuilding`, `deleting`) use `max(for_s, transitional_grace_s)` instead.
- `lb_target_health`: counts every target service (`targets[].health_status[]`, and the nested `targets[]` of
  label-selector targets); `unknown` counts as not healthy, which `for_s` absorbs for new targets. Value = number of bad services.
- `traffic_quota`: servers and load balancers; value = percent used (one decimal). `null` traffic fields → no data, condition false.
  Two default rules instead of one with escalating severity: at ≥ 100 % both are open.
- Field shapes verified against the OpenAPI spec on 2026-11-11.

## Notes and assumptions

- **CPU (from spec, not yet verified):** Hetzner reports per vCPU — 100 = one full core; 400 = full load on 4 vCPUs. Always divide by `cores`. Verify before M3.
- **Traffic forecast (assumption):** the billing period is not in the API. Assume calendar month, linear extrapolation from two snapshots. Label it as an estimate in UI and notifications. Verify before M3.
- **`server_type.deprecation` is deprecated** in the spec — read `server_type.locations[].deprecation` instead.
- **Not possible without an agent:** RAM, disk fill level, processes, logs. Do not fake these.
