# ADR-0006: Multi-tenant data model from day one

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Self-hosted instances often have one user, but the SaaS needs organisations. Retrofitting tenancy is expensive and error-prone.

## Decision

Model organisation → users (memberships with roles) → Hetzner projects from the first migration. Self-hosted gets a default org.

## Consequences

- **Pro:** No schema rewrite for the SaaS.
- **Con:** Every query must be org-scoped; slightly more ceremony in single-user setups.
