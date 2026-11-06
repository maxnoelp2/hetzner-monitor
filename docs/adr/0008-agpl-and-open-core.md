# ADR-0008: AGPL-3.0, all features open source

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

The project should be freely usable and self-hostable, while a hosted SaaS is the business model. MIT would allow anyone to resell a closed fork as a service.

## Decision

License under AGPL-3.0. All features stay open source; the SaaS monetises hosting, convenience and support.

## Consequences

- **Pro:** Network-use clause protects the SaaS from closed competing offerings; builds trust.
- **Con:** Some companies avoid AGPL dependencies; acceptable since this is an application, not a library. If `crates/hcloud` is ever published separately, consider a permissive license for that crate only (new ADR).
