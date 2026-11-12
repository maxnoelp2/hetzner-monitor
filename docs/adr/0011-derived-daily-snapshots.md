# ADR-0011: Store derived daily snapshots for cost and the traffic forecast

- Status: Proposed
- Date: 2026-11-12
- Deciders: Max Nölp (owner), Claude (agent, draft)

## Context

Invariant 8 says "no own time-series storage": metric windows are fetched from Hetzner (30-day retention) whenever a
rule is evaluated ([ADR-0010](0010-polling-tiers-and-budget.md)). Two M3b checks need a value from an **earlier day**
that Hetzner does not keep for us:

- `cost_change` compares today's estimated monthly cost with an earlier day's. The estimate is our own calculation
  (`/pricing` × inventory); Hetzner has no history of it.
- The `traffic_quota` forecast needs two readings of a server's or load balancer's `outgoing_traffic` counter. Hetzner
  only returns the current value. Its `network.0.bandwidth.out` metric is not the same thing: it costs one request
  per server, does not exist for load balancers, and counts all public traffic, not the billed counter.

## Decision

We store two small **derived daily snapshots** in Postgres (migration `0005`), with at most one row per day:

- `cost_estimates`: per project and UTC day, the latest estimated monthly cost of that day (90 days kept).
- `traffic_samples`: per server / load balancer and UTC day, the **first** `outgoing_traffic` reading of that day (40 days kept).

Both are written by inventory cycles in the cycle's transaction and are tenant-scoped through their project. Invariant 8
is reworded to "no own **metric** time-series storage": series that Hetzner keeps (metrics) are still never stored;
daily readings of values Hetzner does not keep are allowed when a check needs them, bounded to one row per day and
a fixed retention.

## Consequences

- **Pro:** `cost_change` and the forecast work without extra Hetzner requests; storage stays tiny (≤ 1 row/day/resource).
- **Pro:** the traffic samples show when a counter actually reset, which could later replace the calendar-month assumption.
- **Con:** two more tables and retention deletes; restarts lose nothing, but a fresh project needs a day before either check has data.
- Rules out: storing per-minute or per-cycle readings, or copying Hetzner's metric series. A new derived table needs the same
  justification (a check needs it, Hetzner does not keep it, one row per day, bounded retention).
