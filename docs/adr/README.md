# Architecture Decision Records

Short documents that record *why* a significant decision was made. Format: [template](0000-template.md).

## Rules

- Number sequentially, never reuse a number. File name: `NNNN-kebab-title.md`.
- An accepted ADR is **not edited in substance** — supersede it with a new ADR and set the old one's status to `Superseded by ADR-XXXX`.
- `Proposed` ADRs may be edited until accepted by a maintainer.
- Agents may draft ADRs as `Proposed`; only a human maintainer sets `Accepted`.

## Index

| ADR | Title | Status |
|---|---|---|
| [0001](0001-cloud-api-only-no-agent.md) | MVP uses the Hetzner Cloud API only, no agent | Accepted |
| [0002](0002-rust-axum-single-binary.md) | Rust + Axum, one binary with run modes | Accepted |
| [0003](0003-postgres-only.md) | PostgreSQL as the only database | Accepted |
| [0004](0004-own-minimal-hetzner-client.md) | Hand-written minimal Hetzner client | Accepted |
| [0005](0005-envelope-encryption-for-secrets.md) | Envelope encryption (AES-256-GCM) for secrets at rest | Accepted |
| [0006](0006-multi-tenant-from-day-one.md) | Multi-tenant data model from day one | Accepted |
| [0007](0007-auth-sessions-and-api-keys.md) | Server-side sessions for the PWA, API keys for headless use | Accepted |
| [0008](0008-agpl-and-open-core.md) | AGPL-3.0, all features open source | Accepted |
| [0009](0009-frontend-react-tauri.md) | React PWA, packaged with Tauri 2 for desktop and mobile | Accepted |
| [0010](0010-polling-tiers-and-budget.md) | Three polling tiers with an adaptive rate-limit budget | Accepted |
| [0011](0011-derived-daily-snapshots.md) | Derived daily snapshots for cost and the traffic forecast | Proposed |
