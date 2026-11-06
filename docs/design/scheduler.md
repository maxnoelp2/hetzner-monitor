# Design: Scheduler & rate-limit budget

Status: **proposed** ([ADR-0010](../adr/0010-polling-tiers-and-budget.md)) · Crate: `crates/server` (loop), `crates/core` (budget maths)

## Problem

Hetzner allows **3600 requests/hour per project**, refilling 1/s, bursts allowed
(as of 2026-11-05, [spec](https://docs.hetzner.cloud/cloud.spec.json)). The budget is shared with
everything else the user runs (Terraform, CI, CLI). Metrics cost **one request per server** per cycle.

## Three tiers

| Tier | Requests | Cost per cycle | Cadence |
|---|---|---|---|
| **fast** | `GET /servers` (status, `locked`, traffic, labels), `GET /load_balancers` (target health) | ⌈N/50⌉ + ⌈L/50⌉ | fixed 60 s |
| **metrics** | `GET /servers/{id}/metrics?type=cpu,disk,network` per server (+ LB metrics) | N (+ L) | **adaptive**, ≥ 60 s |
| **inventory** | volumes, certificates, IPs, server types, pricing | ~5–10 | 15 min |

Status changes are detected within ~1 minute regardless of project size; only utilisation alerts slow down for large projects.

## Budget maths

```
budget_h      = RateLimit-Limit × budget_share          # default 0.5 → 1800/h
fixed_h       = fast_cost × 60 + inventory_cost × 4
metrics_int_s = max(60, ceil(N_metrics × 3600 / (budget_h − fixed_h)))
```

Example, 100 servers: fast = 2 × 60 = 120/h, inventory ≈ 40/h → 1640/h left → metrics every ⌈100·3600/1640⌉ = **220 s**.

If `budget_h − fixed_h ≤ 0`, metrics are paused for the project and a system alert explains why.

## Safeguards

- `RateLimit-Remaining` < 20 % of the limit → double the metrics interval until it recovers (the user is busy elsewhere).
- `budget_share` configurable per project (0.1–0.9).
- Jitter on start so projects don't align.
- On 429: sleep until `RateLimit-Reset`.
- Expose current interval and budget usage per project in the API, so users understand delays.

## Metric query window

`start = now − max(interval, longest rule duration in scope) − 2 min`, `step = 60`.
Rule durations ("CPU > 90 % for 10 min") are evaluated directly on the returned series — **no local time-series storage** (invariant 8).

## Concurrency

- One Tokio task per active project, driven by a central ticker.
- Ownership via Postgres row lock (`SELECT … FOR UPDATE SKIP LOCKED`) or advisory lock, so `HM_MODE=worker` instances can scale out without double polling.
