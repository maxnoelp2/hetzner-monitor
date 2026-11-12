# Plan: M3a — metrics tier and metric checks (`cpu`, `network`, `disk_io`)

- Status: Done
- Milestone: M3 (part a of two; part b = Hetzner-specific, hygiene and cost checks, separate plan and PR)
- Owner: Claude (agent), reviewed by Max Nölp
- Started: 2026-11-11
- PR: —

## Goal

The scheduler gets its third tier: per server it fetches Hetzner's metric series at the adaptive interval from
ADR-0010, and rules like "CPU > 90 % for 10 min" are evaluated **directly on the returned series**, with no metrics
stored locally (invariant 8). Users can create their own rules (needed for the `network` / `disk_io` templates,
which have no default threshold).

## Acceptance criteria

- [x] `hm-hcloud`: `Client::server_metrics(token, id, types, start, end, step)` → typed series (`[ts, "string"]` → `(f64, f64)`;
      unparsable values skipped), rate limit captured. Fixtures hand-written from the spec, anonymised.
- [x] **Metrics tier:** runs only for servers in scope of at least one enabled metric rule; interval =
      `metrics_interval_s(N_metrics, …)` (≥ 60 s, doubling below 20 % remaining, paused if the budget is used up).
      `next_metrics_at` / `last_metrics_at` visible on the project. No metric rule in scope → no metric requests at all.
- [x] **Series evaluation** (pure, `hm-core`): a rule "value > threshold for `for_s`" fires when the samples of the last
      `for_s` seconds all breach and cover the window (gaps > 2 steps break the run). The alert state machine takes the
      duration measured on the series (`held_s`), so a restart or a late cycle neither delays nor fakes a firing.
- [x] **Checks:** `cpu` (value ÷ `server_type.cores`, from the server snapshot — no extra request), `network`
      (bandwidth in/out or pps in/out > threshold), `disk_io` (IOPS or bandwidth read/write > threshold).
      Default rule: `cpu` > 90 % for 10 min, warning. `network` / `disk_io`: templates only (no default rule).
- [x] **Custom rules API:** `POST /rules {project_id, name, check_type, params, severity, selector?, resource_id?}` (write scope,
      params and selector validated, `resource_id` must belong to the project) and `DELETE /rules/{id}` (custom rules only;
      default rules can be disabled, not deleted). Tenant isolation tested.
- [x] **Failures:** metric request errors follow the same rules as other tiers (backoff, `degraded` after 3, `hetzner_api`
      system alert); metric alerts are frozen while the project is not `active` (invariant 6). A server without data
      (new, or Hetzner returns no samples) never fires.
- [x] **End-to-end test** (wiremock, synthetic clock): CPU series at 95 % of 2 cores over 10 min → `firing`; back to 10 % → `resolved`.
- [x] **Real read-only check:** against a real project, a temporary custom rule (e.g. `cpu` > 1 % for 120 s on one server)
      fires from real Hetzner data — no server is touched or stressed. Values plausible vs. the Hetzner Console graph.
- [x] Pre-M3 verification "CPU per vCPU" closed: the spec now states it explicitly (re-checked 2026-11-11); recorded in checks.md and the roadmap.
- [x] fmt, clippy `-D warnings`, tests green; docs updated (checks, scheduler, alerting, api, data-model, hcloud-client, research, roadmap).

## Context

Read on 2026-11-11: roadmap (M3, pre-M3 verifications), ADR-0010, design `scheduler` (metric query window), `checks`,
`hcloud-client`, research `hetzner-cloud-api`; M2 code (`cycle`, `engine`, `scheduler`, `hm_core::{alert, budget, checks}`).

Verified in the [OpenAPI spec](https://docs.hetzner.cloud/cloud.spec.json) on 2026-11-11:
- `GET /servers/{id}/metrics`: `type` (cpu, disk, network; comma-combinable), `start`, `end` required, `step` optional;
  max 500 samples (step adjusted), 30 days retention. Response `metrics.time_series.<name>.values = [[ts, "value"], …]`.
- The spec now says explicitly: CPU "is calculated per vCPU … divide the metric value by the number of vCPUs".
- Every server object embeds `server_type` with `cores` → normalisation costs no request.

Hetzner docs ([billing FAQ](https://docs.hetzner.com/cloud/billing/faq/), 2026-11-11): only outgoing traffic is billed;
invoices cover full calendar months after the first; the owner gets Hetzner's own notice at 75 % and 100 %. The
counter reset date is not stated → the traffic forecast (M3b) keeps "calendar month" as a labelled estimate.

## Approach

1. [x] **Migration `0004_metrics.sql`**: `hetzner_projects.next_metrics_at`; `rules.check_type` CHECK extended by `cpu`, `network`, `disk_io`;
       default `cpu` rule added to `hm_create_default_rules` and inserted for existing projects.
2. [x] **`hm-hcloud::metrics`**: request builder (RFC 3339 `start`/`end`, `step`), response parsing, errors as today. Tests with fixtures.
3. [x] **`hm-core::series`**: `trailing_breach(samples, threshold, step, now) -> Option<held_s>` and window maths
       (`start = now − max(interval, longest for_s) − 2 min`, `step = 60`). Unit tests incl. gaps, empty series, clock skew.
4. [x] **`hm-core::alert`**: `step` takes an optional `held_s` measured by the check; without it the local `pending_since` is used (M2 checks unchanged).
5. [x] **`hm-core::checks`**: `cpu`, `network`, `disk_io` with params (`threshold_pct` / `threshold` + `metric` selector, `for_s`), validation, messages.
6. [x] **Scheduler**: `Tier::Metrics` (priority inventory > fast > metrics; one tier per claim), claim also on `next_metrics_at`,
       budget uses `N_metrics`; cycle fetches only the types the in-scope rules need, one request per server; engine
       evaluates metric rules on metric cycles only.
7. [x] **Rules API**: `POST /rules`, `DELETE /rules/{id}`; docs.
8. [x] **Tests**: unit (core, hcloud), integration (e2e CPU, no-rule-no-requests, budget interval, failure freeze, custom rules + isolation).
9. [x] **Docs** (see acceptance criteria) and a short `scripts/smoke-m3a.sh` for the read-only real check.
10. [x] **Verify**: criteria, owner's read-only real check, record results, open PR.

## Dependencies (new crates)

None planned (RFC 3339 formatting via `time`, already used).

## Out of scope

- M3b: `certificate_expiry`, `deprecation`, hygiene checks, `cost_change` (+ `/pricing`), traffic forecast.
- Load balancer metrics (no check needs them yet).
- Metric graphs / `GET /resources/{id}/metrics` proxy (for the PWA, M6).

## Risks / open questions

- **Budget on the real project:** 9 servers with a metric rule = 9 requests/min ≈ 540/h on top of ~84/h (well inside
  the 1800/h budget, but 15 % of the project's limit, shared with the owner's other tooling). Only servers in scope of
  an enabled metric rule are fetched; disabling the `cpu` default rule stops metric requests.
- Hetzner's newest sample may lag by a minute or two; the window keeps a 2-minute margin, and "no data" never fires.
- `step` is a hint: Hetzner may return a coarser step for long windows; the evaluation uses the step from the response.
- Real acceptance via `stress` is replaced by a read-only check with a low temporary threshold (no production server is loaded).

## Result (2026-11-12)

| Criterion | Result |
|---|---|
| Client, series evaluation, checks, custom rules, failure freeze, e2e CPU | ✅ 104 tests (57 core, 9 hcloud + 1 ignored live, 17 scheduler scenarios) |
| **Real read-only check** (`scripts/smoke-m3a.sh`, owner's real project, 9 servers) | ✅ temporary rule `cpu > 1 % for 120 s` on one server fired in the **first metrics cycle** after it was created (08:20:04 UTC): "CPU 36.4 % of 3 vCPUs for 13 min" — real series parsed, normalised by the snapshot's `cores` (3), duration measured on the series. Default `cpu` rule (90 %) did not fire for any server. Metrics every 60 s, rate limit 3591/3600, no warnings or errors in the app log; rule and project removed afterwards. Comparison with the Console graph: not done by the owner (optional). |
| fmt / clippy `-D warnings --locked` | ✅ |

## Progress log

- 2026-11-11 — Plan drafted; M3 split into M3a (this) and M3b. Spec re-checked: CPU per vCPU now stated explicitly;
  `server_type.cores` embedded in server objects. Billing FAQ read for the traffic period (see Context).
- 2026-11-11 — Approved and implemented. Decisions and deviations: (1) a claim runs every due tier in a row
  (inventory/fast, then metrics) and the **caller** releases the lease — with one tier per claim the metrics tier
  starved behind the 60-s fast tier whenever ticks were sparse. (2) `alert::step` takes a measured duration
  (`held_s`) from metric checks. (3) Metric requests go only to servers in scope of an enabled metric rule, with
  only the needed families; an existing `next_metrics_at` is kept when fast cycles recompute the schedule
  (an earlier draft pulled it forward every minute — caught in review before the tests). (4) A `404` for a
  vanished server is skipped. (5) Metrics paused by the budget have no system alert yet (doc marks it planned).
  (6) `hm-hcloud` now depends on `time` (already in the workspace) for RFC 3339 query parameters.
  Tests: 57 core, 9 hcloud (+1 ignored live), 17 scheduler scenarios incl. 5 new metric ones; 104 in total.
- 2026-11-12 — Owner ran the read-only real check on the M3a stack: passed (see Result). Plan done.
