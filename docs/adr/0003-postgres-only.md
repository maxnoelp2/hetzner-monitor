# ADR-0003: PostgreSQL as the only database

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

SQLite would make self-hosting trivial, but the SaaS needs Postgres. Supporting both doubles the testing surface.

## Decision

Support PostgreSQL only, for self-hosted and SaaS. Docker Compose ships a Postgres container. Migrations via `sqlx migrate`.

## Consequences

- **Pro:** One SQL dialect, `FOR UPDATE SKIP LOCKED`, partial unique indexes, `jsonb`, `citext`.
- **Con:** Self-hosters run a second container.
