# Plan: M3b — inventory, hygiene and cost checks, traffic forecast

- Status: Done ([PR #10](https://github.com/maxnoelp2/hetzner-monitor/pull/10))
- Milestone: M3 (part b of two; part a = [metrics tier](2026-11-11-m3a-metrics-tier.md))
- Owner: Claude (agent), reviewed by Max Nölp
- Started: 2026-11-12
- PR: —

## Goal

The inventory tier evaluates the remaining checks from [checks](../design/checks.md) — `certificate_expiry`,
`deprecation`, `hygiene_backups`, `hygiene_protection`, `hygiene_orphans`, `cost_change` — over all six resource kinds,
and `traffic_quota` gets a clearly labelled calendar-month **forecast**. Nothing new is asked of Hetzner except one
`GET /pricing` per inventory cycle, and no metric series is stored (invariant 8).

## Acceptance criteria

- [x] **Inventory checks run on inventory cycles only** (connect, manual sync, every 15 min) and see all six kinds
      (servers, load balancers, volumes, certificates, floating IPs, primary IPs). Fast cycles leave their alerts untouched.
- [x] `certificate_expiry`: fires when `not_valid_after` is less than `days` away (or already past), or — with
      `on_failure` — when `status.issuance` or `status.renewal` is `failed` (message includes `status.error.message`).
      Uploaded certificates (`status` null) are checked on expiry only; `not_valid_after` null → no data.
- [x] `deprecation`: fires for a server whose `server_type.locations[]` entry for `server.location.name` has a
      `deprecation` (fallback: the deprecated top-level `server_type.deprecation`), and for a load balancer whose
      `load_balancer_type.deprecation` is set. Message names the type, location and `unavailable_after`.
- [x] `hygiene_backups`: server with `backup_window` null. `hygiene_protection`: `protection.delete` (and for servers
      `protection.rebuild`) off, for the kinds in `params.kinds` (default `["server"]`). `hygiene_orphans`: volume with
      `server` null (status `creating` ignored), floating IP with `server` null, primary IP with `assignee_id` null.
      All `info`, with a hold time (`for_s`) so freshly provisioned resources don't alert.
- [x] `cost_change`: project-level. Each inventory cycle computes the estimated monthly cost (net, Hetzner's currency)
      from `/pricing` × inventory and stores it as **today's** estimate; the rule fires when today's estimate is at least
      `threshold_pct` above the latest estimate of an earlier day. First day → no data. Resources without a price are
      counted and named in the message, not guessed.
- [x] `traffic_quota` with `forecast: true`: `outgoing + rate × (end of the calendar month UTC − now)` ≥ `threshold_pct`
      of `included_traffic`, where `rate` comes from a stored daily traffic sample ≥ 24 h old. A counter that went down
      (reset) or a missing sample → no data. The message says "estimate, assuming the quota resets on the 1st (UTC)".
- [x] **Budget:** `inventory_cost` counts the `/pricing` request (+1); `fixed_h` in the API reflects it.
- [x] **Default rules** for new and existing projects (table below); all params validated, unknown fields rejected.
- [x] `GET /projects/{id}` shows the latest `cost_estimate` (`{monthly_net, currency, day, unpriced}` or `null`).
- [x] Tests: unit tests per check and for the cost estimate / forecast maths (fixtures hand-written from the spec,
      anonymised); integration tests (wiremock, synthetic clock) for inventory-only evaluation, cost change across a
      day boundary, forecast across two days, and that fast cycles don't touch inventory alerts.
- [x] **Real read-only check** (`scripts/smoke-m3b.sh`): manual sync on the owner's real project, list the new alerts per
      check and the cost estimate; the owner compares the estimate with the Hetzner Console. Nothing is changed in Hetzner.
- [x] fmt, clippy `--locked -D warnings`, tests green; docs updated (checks, scheduler, alerting, data-model, api,
      research, roadmap, ARCHITECTURE status + invariant 8 wording, CLAUDE.md status); ADR-0011.

## Context

Read on 2026-11-12: `CLAUDE.md`, `ARCHITECTURE.md`, roadmap (M3), design `checks`, `scheduler`, `alerting`,
`data-model`, `api`, the M3a plan; code `hm_core::{checks, alert, series, budget}`, `hm_server::{cycle, engine,
scheduler, inventory, repo::resources}`, `tests/scheduler.rs`, `tests/common`.

Re-checked in the [OpenAPI spec](https://docs.hetzner.cloud/cloud.spec.json) on 2026-11-12:
- `GET /pricing` → `pricing.{currency, vat_rate, volume.price_per_gb_month.{net,gross}, server_backup.percentage,
  server_types[]/load_balancer_types[] .prices[]{location, price_monthly, included_traffic, price_per_tb_traffic},
  primary_ips[]{type, prices[]{location, price_monthly}}, floating_ips[]{type, prices[]{location, price_monthly}}}`;
  prices are decimal strings. Not paginated.
- Server: `server_type.{name, prices[], deprecation (deprecated), locations[]{name, deprecation{unavailable_after, announced}}}`,
  `location.name`, `backup_window` (nullable), `protection.{delete, rebuild}`, `outgoing_traffic`/`included_traffic` (nullable).
- Load balancer: `load_balancer_type.{name, deprecation, prices[]}`, `location.name`, `protection.delete`, traffic fields.
- Certificate: `type` uploaded|managed, `not_valid_after` (nullable), `status` (nullable) `{issuance pending|completed|failed,
  renewal scheduled|pending|failed|unavailable, error{code, message}}`, `domain_names[]`.
- Volume: `server` (nullable), `size` (GB), `location.name`, `status` available|creating, `protection.delete`.
- Floating IP: `type`, `server` (nullable), `home_location.name`, `protection.delete`.
  Primary IP: `type`, `assignee_id` (nullable), `assignee_type` server|unassigned, `location.name`, `protection.delete`.
- Locations carry no deprecation, so "deprecation on location" means the server type's per-location entry.

Billing FAQ (2026-11-11): only outgoing traffic is billed, invoices per calendar month, counter reset date not documented.

## Design decisions

1. **Where inventory checks run.** `CheckType::tier()` (fast / inventory / metrics). `Changes` gets `inventory: bool`;
   on fast cycles the engine evaluates fast checks only and skips inventory rules entirely (no `close_gone`), so their
   alerts keep state. On inventory cycles it loads all six kinds. Hold times (`for_s`) use the alert's local pending
   time, as for the M2 checks. `traffic_quota` without `forecast` stays on the fast tier; with `forecast` it is inventory.
2. **Per-check context.** `CheckType::evaluate` gets a small `Context { now, traffic_baseline }` (pure data, built by the
   engine), so time-dependent checks stay pure and testable.
3. **Cost estimate** (`hm_core::cost`, pure): sum of net monthly prices from `/pricing` — server type by location
   (+ `server_backup.percentage` if `backup_window` is set), load balancer type by location, volume `size` ×
   `price_per_gb_month`, primary IP by type + location, floating IP by type + `home_location`. Not included (documented):
   traffic overage, snapshots/images, object storage. Hourly billing is capped at the monthly price, so the estimate is
   the "full month" figure Hetzner shows.
4. **Two small derived tables** (migration `0005`), justified in **ADR-0011** and reflected in invariant 8's wording
   ("no own *metric* time-series storage"):
   - `cost_estimates(project_id, day, monthly_net, currency, unpriced, at, pk(project_id, day))` — one row per project and
     UTC day, upserted on every inventory cycle (the day's latest value), rows older than 90 days deleted.
   - `traffic_samples(resource_id, day, at, outgoing_traffic, pk(resource_id, day))` — the **first** inventory snapshot of
     each UTC day per server / load balancer, kept 40 days.
   Both are our own derived values that Hetzner does not keep (a metric series it does keep is still never stored), at
   most one row per day, so storage is bounded and tiny. The alternative for the forecast — Hetzner's
   `network.0.bandwidth.out` series — costs one request per server, does not exist for load balancers, and covers
   all public traffic rather than the billed counter.
5. **Forecast baseline:** the newest sample at least 24 h old (so the rate spans 24–48 h and daily patterns average out).
   `rate = Δoutgoing / Δt`; if the counter is lower than the sample, it was reset → no data until the next baseline.
6. **Pricing request:** fetched on every inventory cycle (one request, `budget::inventory_cost` + 1) so the cost estimate
   is always current; a `/pricing` error fails the cycle like any other listing.
7. **Severity per rule, not per check:** checks with two severities get two default rules (as `traffic_quota` does).

### Default rules (added to `hm_create_default_rules` and inserted for existing projects)

| Rule | `check_type` | Params | Severity |
|---|---|---|---|
| Certificate expiring or renewal failed | `certificate_expiry` | `days` 14, `on_failure` true | warning |
| Certificate expires within 3 days | `certificate_expiry` | `days` 3, `on_failure` false | critical |
| Type deprecated | `deprecation` | — | info |
| Server backups disabled | `hygiene_backups` | `for_s` 3600 | info |
| Delete/rebuild protection off | `hygiene_protection` | `kinds` `["server"]`, `for_s` 3600 | info |
| Unattached volume or IP | `hygiene_orphans` | `for_s` 86400 | info |
| Estimated cost increased | `cost_change` | `threshold_pct` 10 | info |
| Traffic quota forecast exceeded | `traffic_quota` | `threshold_pct` 100, `forecast` true | warning |

**Noise:** hygiene rules are enabled by default but `info` only; they fire once per resource and stay open while the
finding holds. Opt-out per resource via the rule's label selector (e.g. `PATCH` selector `backups!=off`) or by disabling
the rule. Re-notify behaviour for `info` is decided in M4 (proposal noted there: no re-notify for `info`).

## Approach

1. [x] **Migration `0005_inventory_checks.sql`**: `check_type` CHECK extended; `cost_estimates`, `traffic_samples`;
       `hm_create_default_rules` replaced; new default rules for existing projects.
2. [x] **`hm-hcloud`**: `Client::pricing(token) -> Priced { raw: Value, rate_limit }` (GET only), fixture test.
3. [x] **`hm-core::cost`**: `estimate(pricing, resources) -> Estimate { monthly_net, currency, unpriced: Vec<label> }`; unit tests.
4. [x] **`hm-core::checks`**: new check types, params, `tier()`, `Context`, forecast in `traffic_quota`; unit tests
       (expiry boundaries, failed renewal, uploaded certs, deprecation per location + fallback, LB, hygiene per kind,
       cost increase, forecast incl. reset and month end).
5. [x] **`hm-core::budget`**: pricing request in `inventory_cost`.
6. [x] **Server**: cycle fetches `/pricing` on inventory cycles, writes the cost estimate and traffic samples in the
       cycle's transaction; engine evaluates inventory checks (all kinds) on inventory cycles; project view shows `cost_estimate`.
7. [x] **Tests**: integration scenarios (see acceptance criteria); existing tests adjusted (`fixed_h`, default rule list, `/pricing` mock).
8. [x] **Docs + ADR-0011**, `scripts/smoke-m3b.sh` (read-only, macOS Python 3.9).
9. [x] **Verify**: criteria, open PR, offer the owner's read-only real check, record the result.

## Dependencies (new crates)

None.

## Out of scope

- Image deprecation (`server.image.deprecation`): affects rebuilds, not running servers — later, if wanted.
- Traffic overage cost, snapshot/image/object-storage cost in the estimate.
- Detecting the real traffic reset date (the samples would allow it later: a counter drop marks a reset).
- Notifications and re-notify policy (M4); the "metrics paused by budget" system alert.

## Risks / open questions

- **Hygiene noise on the real project:** production servers without backups or protection will show `info` alerts at
  once (after `for_s`). Intended — that is the finding — but the owner may want different defaults (e.g. created disabled).
- **Pricing gross vs. net:** the estimate uses net prices (what the Console lists by default); VAT differs per customer.
- **Cost change on day one** has no baseline; the first comparison happens on the second UTC day after connect.
- **Forecast accuracy:** linear over 24–48 h, calendar month assumed; spiky traffic can over- or under-shoot. Labelled as an estimate.

## Progress log

- 2026-11-12 — Plan drafted; spec re-checked (pricing, certificates, volumes, IPs, LB types, server type locations).
- 2026-11-12 — Approved by the owner and implemented as planned. Details and deviations: (1) `CheckType::inventory_only(params)`
  instead of a tier enum, because `traffic_quota` is fast or inventory depending on `forecast`. (2) `CheckType::evaluate`
  takes a `Context { now, kind, traffic_baseline }`; `cost_change` has its own `evaluate_cost`. (3) `/pricing` is
  returned raw by `hm-hcloud` (`Priced`), interpreted in the new `hm_core::cost`; server/LB prices fall back to the
  `prices` embedded in the snapshot. (4) `hm-core` now uses `time` (already a workspace dependency) for RFC 3339 dates
  and the month end. (5) `GET /projects/{id}` gets `cost_estimate`; the list endpoint does not. (6) `cycle::run` and
  `engine::evaluate` were split into helpers (`fetch`, `evaluate_cost`, `evaluate_metrics`) to stay under clippy's
  100-line limit. Tests: 69 core, 10 hcloud (+1 ignored live), 4 new integration scenarios in `tests/inventory_checks.rs`,
  existing scenarios adjusted (`fixed_h` + `/pricing`, default rule list, migration table list).
- 2026-11-12 — Read-only real check on the owner's project (`scripts/smoke-m3b.sh`, temporary project, disconnected
  again): 9 servers. `deprecation` fired for 4 servers (cpx11/cpx21/cpx31, deprecated 2025-10-16); `hygiene_backups`
  pending for 5 and `hygiene_protection` for 9 servers, matching the real backup flags; no certificates, orphans,
  `cost_change` or forecast (expected on day one). Budget 88 of 1800 requests/h. Two fixes from the run: (1) the first
  estimate left 8 resources unpriced — a read-only diagnosis (`scripts/diag-pricing.py`) showed they were the primary
  IPv6s: the real `/pricing` lists only `ipv4` under `primary_ips` (primary IPv6 is free), so an unlisted `ipv6` now counts
  as 0 and unpriced labels are logged; (2) past `unavailable_after` dates now read "unavailable for new resources since".
  Second run: estimate 81.00 EUR net with nothing unpriced. The smoke script also checks the M3b rule only after connect
  (default rules are per project) and reads missing keys from `deploy/.env`.
