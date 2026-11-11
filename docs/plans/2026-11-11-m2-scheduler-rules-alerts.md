# Plan: M2 — scheduler, rule engine, alert lifecycle, first checks

- Status: Done — real power-off test skipped by owner decision (see Result)
- Milestone: M2
- Owner: Claude (agent), reviewed by Max Nölp
- Started: 2026-11-11
- PR: —

## Goal

Connected projects are polled on their own: the fast tier every 60 s and the inventory tier every 15 min,
both inside the rate-limit budget. Enabled rules are evaluated on every cycle, and alerts move through
`pending → firing → resolved`. Powering off a server produces a `firing` alert, visible via the API;
powering it on resolves it. Nothing is sent to channels yet: notifications are M4, so M2 only writes `alert_events`.

## Acceptance criteria

- [x] **Scheduler:** with `HM_MODE=all|worker`, every `active` project gets a fast cycle about every 60 s and an inventory cycle about every 15 min, with start jitter. Two scheduler instances never poll the same project at the same time (lease test).
- [x] **Budget:** `hm-core::budget` implements the maths from [scheduler](../design/scheduler.md#budget-maths), unit-tested against the 100-server example (220 s). `GET /projects/{id}` shows `metrics_interval_s`, `budget_h`, `fixed_h` and the time of the next fast and inventory cycle.
- [x] **Hetzner errors:** `429` → next cycle not before `RateLimit-Reset`. `401` → project `invalid_token`, polling stops. `403`/`maintenance`/5xx/network → `degraded` after 3 consecutive failures, with backoff (60 s → 15 min), and back to `active` on the next success.
- [x] **Invariant 6:** while a project is `degraded` or `invalid_token`, no availability alert of that project changes state, and exactly one system alert (`hetzner_api`) is `firing`. It resolves when the project is `active` again.
- [x] **Rules:** on connect (and for existing projects via the migration), default rules are created for `server_status`, `resource_lifecycle`, `lb_target_health`, `traffic_quota` (80 % warning, 100 % critical) and `hetzner_api`. Scope can be project, resource or label selector (`env=prod,role!=db`; parser in `hm-core` covers the Hetzner selector syntax `=`, `==`, `!=`, `in (…)`, `notin (…)`, `k`, `!k`).
- [x] **State machine** (pure, in `hm-core`): `ok → pending → firing` after the rule's `for` duration, `pending → ok`, `firing → resolved` only after 2 consecutive good cycles. At most one open alert per (rule, resource) through a partial unique index. Every transition writes an `alert_events` row.
- [x] **Checks** (pure, fixtures from the spec): `server_status` (≠ `running` for ≥ 2 cycles; `starting`/`migrating`/`rebuilding` tolerated for < 10 min), `lb_target_health` (any target service ≠ `healthy`), `traffic_quota` (`outgoing_traffic / included_traffic` for servers and LBs), `resource_lifecycle` (created/deleted from the inventory diff, as a one-shot event).
- [x] **API:** `GET /rules`, `PATCH /rules/{id}` (`enabled`, `params`, `severity`, `selector`), `GET /alerts?state=&project_id=`, `GET /alerts/{id}` (with events). Tenant-scoped, isolation tested. Creating and deleting rules is out of scope (see below).
- [x] **End-to-end test** (wiremock Hetzner, injected clock): server `running` → `off` over several cycles → alert `firing` → `running` → `resolved` after 2 cycles.
- [ ] ~~**Real test by the owner**~~ — **not performed** (owner decision, 2026-11-11): the owner's only connected project runs production servers, which must not be powered off, and no separate test project was set up. Covered instead by the integration test `power_off_fires_and_power_on_resolves` against a mocked Hetzner API.
- [x] fmt, clippy `-D warnings`, tests green; docs updated (scheduler, alerting, checks, data-model, api, configuration, ARCHITECTURE status, CLAUDE.md status, roadmap); ADR-0010 → Accepted.

## Context

Read on 2026-11-11: roadmap (M2), ARCHITECTURE (data flow, invariants 2, 6, 7, 8), ADR-0010, design `scheduler`,
`alerting`, `checks`, `data-model`, `api`, `hcloud-client`, research `hetzner-cloud-api`. Code: `hm_server::inventory`
(connect + sync of all 6 kinds, project status `active|invalid_token|degraded`), `repo::{projects,resources}`,
`hm_hcloud::Client::list` (GET only, `RateLimit` per response), migration `0002` (`hetzner_projects` already has
`budget_share`, `rl_*`, `metrics_interval_s`, `last_fast_at`, `last_inventory_at`).

The Hetzner field shapes used by the checks (`servers[].status`, `outgoing_traffic`, `included_traffic`,
`load_balancers[].targets[].health_status[].status`, LB traffic fields) are re-checked against the
[spec](https://docs.hetzner.cloud/cloud.spec.json) in step 5, before the checks are written.

## Approach

1. [x] **Migration `0003_rules_alerts.sql`**: `rules`, `alerts`, `alert_events` as in [data-model](../design/data-model.md) with `CHECK`s;
       partial unique index `(rule_id, coalesce(resource_id, nil-uuid)) WHERE state <> 'resolved'`; `alerts.ok_cycles int`
       for the flapping guard. `hetzner_projects` gets `next_fast_at`, `next_inventory_at`, `lease_until`, `failures int`.
       Default rules for existing projects are inserted in the migration.
2. [x] **`hm-core::budget`**: pure functions `budget_h`, `fixed_h`, `metrics_interval_s` (with N = 0 in M2), the
       low-remaining doubling, and the backoff schedule. Unit tests.
3. [x] **`hm-core::selector`**: parser + matcher for Hetzner label selectors over `labels: Map<String, Value>`. Unit tests incl. invalid input.
4. [x] **`hm-core::alert`**: state machine `step(prev, condition, now, params) -> (next, Vec<Event>)`; `for` duration, flapping guard,
       one-shot events (for `resource_lifecycle`), "resource gone" → resolve. Unit tests for every edge in the diagram.
5. [x] **`hm-core::checks`**: `server_status`, `lb_target_health`, `traffic_quota` on `serde_json::Value` snapshots
       (typed views via serde, unknown fields ignored); `resource_lifecycle` on the `KindCounts` diff with resource lists.
       Fixtures hand-written from the spec.
6. [x] **Repositories**: `repo::rules` (list in scope, defaults on connect, patch), `repo::alerts` (open alerts per rule,
       upsert transition + event in one transaction, list/get). All take `OrgId`.
7. [x] **Scheduler (`hm-server::scheduler`)**: a central ticker (5 s) claims due projects with
       `UPDATE … SET lease_until = now() + 2 min WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED) RETURNING …`;
       the row lock is only held for the claim, never across a Hetzner request. Per claimed project: run the due tier
       (fast = `servers` + `load_balancers`; inventory = the existing sync of all 6 kinds), update `resources`,
       compute `next_*_at` from the budget, evaluate rules, release the lease. Clock behind a trait, so tests can drive cycles.
       Started from `serve` in `all`/`worker` mode; graceful shutdown on SIGTERM.
8. [x] **Rule engine (`hm-server::engine`)**: for each enabled rule in scope → resources (project/resource/selector) → check →
       `alert::step` → persist. Suspends availability checks while the project is not `active`; drives the `hetzner_api` system alert.
9. [x] **API**: rules and alerts endpoints; extend the project view with budget fields. Error codes as in [api](../design/api.md#conventions).
10. [x] **Tests**: unit tests (core), integration with `#[sqlx::test]` + wiremock: e2e power off/on, 429/401/5xx behaviour,
        invariant 6, lease exclusivity, tenant isolation of rules/alerts, defaults on connect.
11. [x] **Smoke script `scripts/smoke-m2.sh`**: connects a project with `--keep`, then polls `GET /alerts` every 15 s and prints
        state changes, so the owner can watch the power off/on test.
12. [x] **Docs**: see acceptance criteria. Record the lease design in scheduler.md (it refines "row lock").
13. [x] **Verify**: all criteria except the real power-off test (skipped, see Result); PR opened.

## Dependencies (new crates)

None planned. Jitter uses `getrandom` (already in the tree); the selector parser is hand-written (small grammar).

## Out of scope

- **Metrics tier and metric checks** (`cpu`, `network`, `disk_io`): M3. The budget maths already reserves for them.
- **Notifications, channels, ack, silences, re-notify**: M4. `renotify_s` and `acked_*` columns are created but unused.
- `POST /rules` / `DELETE /rules/{id}` (custom rules): only after the check params are stable; defaults can be edited and disabled.
- Traffic **forecast** (needs the billing-period assumption, pre-M3 verification): M2 only uses the current ratio.
- `certificate_expiry`, `deprecation`, hygiene, cost checks: M3.
- `/metrics` Prometheus endpoint for own operational metrics.

## Risks / open questions

- **The owner's real test must not run on production servers.** The connected project has production servers; for the
  power-off test, use a separate test project with one small server (a few cents per hour) or a dedicated test server in it.
- `resource_lifecycle` is an event, not a condition. The plan models it as an alert that is created `firing` and
  `resolved` in the same cycle (two events), so M4 can notify it like any other alert. Alternative: a separate event table.
- `server_status` "for ≥ 2 cycles" vs. rule `for` duration: modelled as `for_s = 120` (2 fast cycles), so all rules use one mechanism.
- `traffic_quota` with two severities = two default rules (80 % warning, 100 % critical) instead of one rule with escalating severity: simpler, but two alerts at ≥ 100 %.
- Lease instead of a long-held row lock: crash safety relies on `lease_until` expiring (2 min).
- Inventory sync rewrites all resource snapshots every 15 min; fine at this size, revisit with many projects.

## Result (2026-11-11)

| Criterion | Result |
|---|---|
| Scheduler, lease, budget, Hetzner errors, invariant 6, rules, state machine, checks, API | ✅ 43 core unit tests + 12 scheduler integration scenarios (wiremock Hetzner, synthetic clock) |
| End-to-end power off → `firing` → power on → `resolved` | ✅ `power_off_fires_and_power_on_resolves` (mocked Hetzner) |
| Real scheduler loop | ✅ binary started against a local DB, scheduler started, clean stop on SIGTERM |
| Real stack with M2 against the real Hetzner API | ✅ owner's stack rebuilt with M2, real project (9 servers, read-only): connect, 6 default rules, fast cycles every ~60 s, `failures` 0, `fixed_h` 84 (= 9 servers · 60 + 6 listings · 4), `budget_h` 1800, no alerts (all servers running) |
| **Real power-off test** | ⏭️ **not performed — owner decision.** The available project runs production servers (must not be powered off); no test project. Gap: the real `status` values and timing of a power-off are not observed against Hetzner itself, only the documented spec values in the mock. Can be done later with `scripts/smoke-m2.sh` and a dedicated test project. |
| fmt / clippy `-D warnings --locked` / tests | ✅ 83 tests |

## Progress log

- 2026-11-11 — Plan drafted after M1 was closed (PR #7); approved by Max.
- 2026-11-11 — Implemented. Decisions and deviations: (1) a never-fired `pending` alert is **deleted** on `pending → ok`
  instead of being kept, so "every transition writes an event" holds for notifiable transitions only. (2) Hold times
  are a per-outcome `hold_s`, so `server_status` can give transitional statuses a longer grace without a second mechanism.
  (3) The fast tier skips `GET /load_balancers` while a project has none (matches ⌈L/50⌉ = 0 in the design); new load
  balancers appear with the next inventory cycle. (4) A 429 is not counted as a failure and sets `rl_remaining = 0`.
  (5) Default rules live in one SQL function used by the migration and by connect. (6) Connect and manual sync take the
  scheduler's lease; manual sync while a cycle runs → `409 sync_in_progress`. (7) `metrics_interval_s` is already
  computed and shown (planned interval for M3). (8) Hetzner spec re-checked: label-selector LB targets nest their
  health in `targets[].targets[].health_status`. Tests: 43 core unit tests, 12 scheduler integration scenarios;
  the real loop was started against a local DB and stopped cleanly on SIGTERM.
- 2026-11-11 — Owner's real test: the stack first still ran M1 (no `/alerts`), and the token in the shell was the
  production one, so `smoke-m2.sh` listed production servers. Hardened the script: preflight check that the stack is
  M2, a confirmation prompt before watching, the project is disconnected again on "no", HTTP errors no longer crash
  the watch loop. The owner then decided to skip the real power-off test (production servers must not be powered off);
  recorded under Result. Plan done.
