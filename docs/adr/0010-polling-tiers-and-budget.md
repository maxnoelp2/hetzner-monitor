# ADR-0010: Three polling tiers with an adaptive rate-limit budget

- Status: Proposed
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Hetzner allows 3600 requests/hour/project, shared with the user's other tooling. Metrics cost one request per server; status for 50 servers costs one request.

## Decision

Poll in three tiers — fast (60 s fixed), metrics (adaptive, ≥ 60 s), inventory (15 min) — using at most `budget_share` (default 0.5) of `RateLimit-Limit`. Evaluate rule durations on Hetzner's own time series; store no metrics locally. Details: [scheduler](../design/scheduler.md).

## Consequences

- **Pro:** Status alerts stay fast for any project size; no TSDB to operate.
- **Con:** Utilisation alerts get slower for large projects (e.g. 220 s at 100 servers); users must see why (API exposes interval and budget).
