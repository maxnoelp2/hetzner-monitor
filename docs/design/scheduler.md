# Design: Scheduler & rate-limit budget

Status: fast and inventory tiers, budget maths, error handling and leases **implemented** (M2); metrics tier **implemented** (M3a); `/pricing` and daily snapshots on the inventory tier **implemented** (M3b); load balancer metrics **proposed** · [ADR-0010](../adr/0010-polling-tiers-and-budget.md) · Crates: `crates/server` (`scheduler`, `cycle`), `crates/core` (`budget`)

## Problem

Hetzner allows **3600 requests/hour per project**, refilling 1/s, bursts allowed
(as of 2026-11-05, [spec](https://docs.hetzner.cloud/cloud.spec.json)). The budget is shared with
everything else the user runs (Terraform, CI, CLI). Metrics cost **one request per server** per cycle.

## Three tiers

| Tier | Requests | Cost per cycle | Cadence |
|---|---|---|---|
| **fast** | `GET /servers` (status, `locked`, traffic, labels), `GET /load_balancers` (target health; skipped while the project has none) | ⌈N/50⌉ + ⌈L/50⌉ | fixed 60 s (+ 0–5 s jitter) |
| **metrics** | `GET /servers/{id}/metrics?type=…` per server **in scope of an enabled metric rule**, only the families those rules need (LB metrics: later) | N_metrics | **adaptive**, ≥ 60 s (+ 0–5 s jitter) |
| **inventory** | all six resource kinds + `GET /pricing` (M3b) | Σ ⌈n_kind/50⌉ + 1 (≥ 7) | 15 min |

Status changes are detected within ~1 minute regardless of project size; only utilisation alerts slow down for large projects.

## Budget maths

```
budget_h      = RateLimit-Limit × budget_share          # default 0.5 → 1800/h
fixed_h       = fast_cost × 60 + inventory_cost × 4
metrics_int_s = max(60, ceil(N_metrics × 3600 / (budget_h − fixed_h)))
```

Example, 100 servers: fast = 2 × 60 = 120/h, inventory ≈ 40/h → 1640/h left → metrics every ⌈100·3600/1640⌉ = **220 s**.

If `budget_h − fixed_h ≤ 0`, metrics are paused for the project (`metrics_interval_s` = `null`); a system alert explaining it is **planned**.

An empty list still costs one request, so every inventory listing counts at least 1; `/pricing` adds one per inventory cycle.

**Inventory cycles** also store the derived daily snapshots ([ADR-0011](../adr/0011-derived-daily-snapshots.md)) in the cycle's
transaction — today's cost estimate (`cost_estimates`, upserted) and the day's first traffic counter per server / load
balancer (`traffic_samples`) — and then evaluate the inventory-only checks ([checks](checks.md#inventory-checks-m3b)).
Fast cycles skip those rules.
`N_metrics` counts only servers covered by an enabled metric rule (`cpu`, `network`, `disk_io`); with none, the metrics
tier does not run at all and `next_metrics_at` is `null`. Disabling the default `cpu` rule therefore stops all metric requests.
`GET /projects/{id}` shows `budget_h`, `fixed_h`, `metrics_interval_s` (`null` = no metric rule in scope, or paused),
`next_fast_at`, `next_inventory_at`, `last_metrics_at` and `next_metrics_at`.

**Order:** a claim runs every due tier in a row under one lease — inventory (which includes the fast tier's data) or
fast, then metrics — and releases the lease at the end. A failing tier stops the rest; its backoff postpones all tiers.

## Safeguards

- `RateLimit-Remaining` < 20 % of the limit → double the metrics interval until it recovers (the user is busy elsewhere).
- `budget_share` configurable per project (0.1–0.9).
- Jitter (0–5 s) on every schedule so projects don't align.
- On 429: no cycle of either tier before `RateLimit-Reset` (at least 60 s); not counted as a failure.
- Expose current interval and budget usage per project in the API, so users understand delays.

## Failures

| Hetzner answer | Project | Next attempt |
|---|---|---|
| 401 | `invalid_token` immediately; the scheduler stops polling it | only after a successful manual `POST /projects/{id}/sync` |
| 403 / maintenance / 5xx / network | `failures + 1`; `degraded` from 3 consecutive failures | backoff 60 s, doubling, max 15 min (both tiers) |
| 429 | unchanged | at `RateLimit-Reset` |
| success | `active`, `failures = 0` | normal cadence |

A failed cycle evaluates only the `hetzner_api` system rule ([alerting](alerting.md#data-source-failure-invariant-6)).
A cycle — fetch, `resources` update, schedule, rule evaluation — commits in one transaction.

## Metric query window

`start = now − max(interval, longest rule duration in scope) − 2 min`, `end = now`, `step = 60` (`hm_core::series::window`).
Rule durations ("CPU > 90 % for 10 min") are evaluated directly on the returned series — **no local time-series storage** (invariant 8):

- The trailing run of samples meeting the condition counts one `step` per sample (Hetzner may return a coarser step than requested; the returned one is used).
- A gap larger than 2 × step ends the run. No samples, or a newest sample older than `max(3 × step, 5 min)` (e.g. a powered-off server), means **no data**, which never fires.
- The measured duration goes into the alert state machine (`held_s`), so restarts or late cycles neither delay nor fake a firing.
- A server deleted since the last inventory answers `404`; it is skipped, not counted as a failure.

## Concurrency

- A central ticker (every 5 s) claims due `active`/`degraded` projects and runs one Tokio task per claimed project
  (at most 16 in flight per instance), so a slow project never delays the others.
- **Lease:** the claim is `UPDATE … SET lease_until = now + 5 min WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED)`.
  The row lock lives only for that statement — never across a Hetzner request; the lease is the ownership.
  A finished cycle clears `lease_until`; if a worker dies, the lease expires and another instance takes over.
  `HM_MODE=worker` instances can therefore scale out without double polling.
- Connect and manual sync take the same lease; a manual sync during a running cycle gets `409 sync_in_progress`.
- The scheduler runs in `HM_MODE=all` and `worker`, not in `api`. On SIGTERM it stops claiming and waits up to 25 s for running cycles.
