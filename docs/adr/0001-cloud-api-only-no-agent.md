# ADR-0001: MVP uses the Hetzner Cloud API only, no agent

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Users want alerts for Hetzner Cloud servers with minimal setup. An agent gives richer data (RAM, disk fill level, services) but requires installation on every server and a much larger trust surface.

## Decision

The MVP reads exclusively from the Hetzner Cloud API using a per-project read-only token. No agent, no external probes.

## Consequences

- **Pro:** Setup in minutes, zero footprint on servers, read-only token is low risk.
- **Con:** No RAM, disk fill level, processes, logs or private-network traffic. Must be communicated clearly ([product](../product.md#known-limits-communicate-clearly-to-users)).
- **Con:** An optional agent and external HTTP/ping/SSL checks are later phases.
