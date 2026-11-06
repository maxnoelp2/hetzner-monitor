# CLAUDE.md

Guidance for AI coding agents (Claude Code and others) working in this repository.
Read this file first, then [ARCHITECTURE.md](ARCHITECTURE.md), then the doc relevant to your task in [docs/](docs/README.md).

## What this project is

**hetzner-monitor** is an open-source (AGPL-3.0) monitoring and alerting tool for **Hetzner Cloud**.
A user connects a Hetzner Cloud project with a **read-only API token** and gets useful alerts —
no agent on the servers, no Prometheus stack.

- Core: one Rust binary (Axum HTTP API + Tokio scheduler) backed by PostgreSQL.
- Later: React PWA → Tauri 2 mobile → Tauri 2 desktop, and a hosted SaaS (open core).

## Current status

**Pre-code.** Only documentation exists. Next milestone is **M0** (workspace skeleton, Docker Compose, CI).
See [docs/roadmap.md](docs/roadmap.md) for milestones and their acceptance criteria.

## Where things are

| You need … | Read |
|---|---|
| Big picture, components, invariants | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Product scope, MVP features, non-goals | [docs/product.md](docs/product.md) |
| Why a decision was made | [docs/adr/](docs/adr/README.md) |
| Design of a subsystem | [docs/design/](docs/README.md#design) |
| Hetzner Cloud API facts and limits | [docs/research/hetzner-cloud-api.md](docs/research/hetzner-cloud-api.md) |
| Terms (tier, budget, DEK, silence …) | [docs/glossary.md](docs/glossary.md) |
| Work in progress / execution plans | [docs/plans/](docs/plans/README.md) |

## Commands

> Not available yet — they arrive with M0. Update this section when they exist.

```bash
cargo fmt --all                                   # format
cargo clippy --all-targets --all-features -- -D warnings   # lint
cargo test --all                                  # test (needs Postgres, see below)
docker compose -f deploy/docker-compose.yml up    # run locally
```

## How to work here

1. **Plan before non-trivial work.** For anything beyond a small fix, write an execution plan in
   `docs/plans/` using the [template](docs/plans/TEMPLATE.md) and keep it updated while you work.
2. **Decisions go into ADRs.** If you change or make an architectural decision, add an ADR
   (`docs/adr/NNNN-title.md`, from the [template](docs/adr/0000-template.md)). Never silently
   contradict an accepted ADR — propose a superseding one instead.
3. **Docs are part of the change.** If behaviour, config, schema or API changes, update the
   matching doc in the same PR. A design doc that disagrees with the code is a bug.
4. **Verify against reality.** Read the actual code, schema and Hetzner API spec before claiming
   something is a bug or designing around it. Hetzner facts carry a date — re-check them if they
   matter (`https://docs.hetzner.cloud/cloud.spec.json`).
5. **Small, reviewable PRs.** One concern per PR. Conventional commit messages
   (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
6. **Definition of done:** fmt + clippy clean, tests pass, docs updated, roadmap/plan checkbox ticked.

## Hard rules

- **Never call Hetzner write endpoints.** This tool is read-only by design. The `hcloud` crate
  only implements `GET` requests. Do not add `POST`/`PUT`/`DELETE` to it.
- **Never log, print, return or commit secrets**: Hetzner tokens, webhook URLs, master keys,
  API keys, session tokens. Use the redaction helpers; add new secret fields to them.
- **Secrets at rest are always encrypted** with the envelope scheme in
  [docs/design/security.md](docs/design/security.md). No plaintext secret columns.
- **Respect the rate-limit budget.** Every Hetzner request goes through the client's budget
  accounting. No ad-hoc `reqwest` calls to `api.hetzner.cloud`.
- **Every query is tenant-scoped.** Queries on tenant data must filter by `org_id`
  (directly or via a join). Cross-tenant leaks are the worst bug class here.
- **A Hetzner API outage must never produce "server down" alerts.** See
  [docs/design/alerting.md](docs/design/alerting.md).
- **No real tokens in tests or fixtures.** Recorded fixtures must be anonymised.
- Don't add dependencies casually; justify new crates in the PR description.

## Conventions

- Language for code, comments, docs, commits: **English**.
- Rust: edition 2024, `rustls` only (no OpenSSL), `thiserror` in libraries, `anyhow` only in the
  binary, `tracing` for logs (structured fields, no `println!`).
- Errors are typed at crate boundaries; map Hetzner error codes explicitly.
- Config only via `HM_*` environment variables (see [docs/design/configuration.md](docs/design/configuration.md)).
- Time is UTC everywhere (`timestamptz`, `time`/`chrono` UTC types).
