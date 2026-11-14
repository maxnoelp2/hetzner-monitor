# Documentation

Docs are the source of truth for **intent** (what and why); code and migrations are the source of
truth for **behaviour**. When they disagree, fix one of them in the same PR.

## Map

| Doc | Purpose |
|---|---|
| [product.md](product.md) | Vision, target users, MVP scope, non-goals |
| [roadmap.md](roadmap.md) | Milestones M0–M6 with acceptance criteria |
| [glossary.md](glossary.md) | Shared vocabulary |
| [adr/](adr/README.md) | Architecture Decision Records — *why* things are the way they are |
| [research/](research/) | External facts (Hetzner API, competitors), always dated |
| [plans/](plans/README.md) | Execution plans for work in progress |

### Design

| Doc | Covers |
|---|---|
| [design/hcloud-client.md](design/hcloud-client.md) | Hetzner API client: endpoints, pagination, errors, rate-limit headers |
| [design/scheduler.md](design/scheduler.md) | Polling tiers, rate-limit budget, adaptive intervals |
| [design/checks.md](design/checks.md) | All check types, defaults, data sources |
| [design/alerting.md](design/alerting.md) | Rules, alert state machine, silences, notifications |
| [design/data-model.md](design/data-model.md) | PostgreSQL schema |
| [design/security.md](design/security.md) | Envelope encryption, auth, SSRF, redaction |
| [design/api.md](design/api.md) | HTTP API surface |
| [design/configuration.md](design/configuration.md) | `HM_*` environment variables |
| [design/web.md](design/web.md) | Web UI (PWA): structure, routing, auth flow, serving and headers, service worker, build |

## Writing rules

- English, concise, present tense for decided things, "**Proposed:**" prefix for undecided ones.
- External facts carry a date and a source URL: *"3600 requests/hour/project (as of 2026-11-05, [spec](https://docs.hetzner.cloud/cloud.spec.json))"*.
- Mark uncertainty explicitly: **verified**, **from spec**, **assumption**.
- Link instead of duplicating. Each fact has one home; other docs link to it.
