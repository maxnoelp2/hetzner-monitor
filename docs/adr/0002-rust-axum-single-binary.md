# ADR-0002: Rust + Axum, one binary with run modes

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

The core is a long-running poller plus an HTTP API. It must be easy to self-host (small image, one process) and scale out later for the SaaS.

## Decision

Write the core in Rust with Axum and Tokio. Ship one binary; `HM_MODE=all|api|worker` selects which subsystems run. Default `all`.

## Consequences

- **Pro:** Small static binary/image, low memory, strong typing for secret handling.
- **Pro:** Same artefact for self-hosted and SaaS.
- **Con:** Slower iteration than Go/Python; no official Hetzner SDK for Rust (see ADR-0004).
- **Con:** Worker coordination needed for scale-out (row locks, see [scheduler](../design/scheduler.md#concurrency)).
