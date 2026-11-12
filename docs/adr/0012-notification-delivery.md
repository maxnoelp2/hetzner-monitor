# ADR-0012: Deliver notifications at-least-once from a Postgres queue

- Status: Accepted
- Date: 2026-11-12
- Deciders: Max Nölp (owner), Claude (agent, draft)

## Context

Invariant 9 says notifications are durable: a restart must lose nothing. Alerts change state inside a poll cycle's
transaction ([ADR-0010](0010-polling-tiers-and-budget.md)); sending to Slack, an SMTP server or a webhook happens
outside our control, can be slow, rate-limited (`429`) or down. We run a single binary on Postgres only
([ADR-0003](0003-postgres-only.md)), so there is no message broker.

## Decision

- **Enqueue in the alert's transaction.** Whenever an alert event `fired`, `resolved` or `renotified` is written, the
  same transaction evaluates silences and routing and inserts one `notification_jobs` row per channel (or a
  `suppressed` event). A cycle that rolls back enqueues nothing; a committed transition always has its jobs.
- **Routing and silences are decided at enqueue time.** A job is a commitment to deliver; changing a channel's
  `min_severity` or creating a silence later does not recall queued jobs. A `resolved` goes only to the channels its
  `fired`/`renotified` went to. One-shot alerts (`resource_lifecycle`, fired and resolved in one cycle) notify once.
- **One worker loop sends jobs while holding the row lock** (`SELECT … FOR UPDATE SKIP LOCKED`, short send timeout),
  then marks them `sent`, re-queues with exponential backoff (honouring `Retry-After`) or marks them `failed` after a
  fixed number of attempts or a permanent error. Several workers can run without double-claiming a job.
- **Delivery is at-least-once.** A crash after the remote accepted a message but before the commit re-sends it once.
  Webhooks carry the job id (`X-HM-Delivery`) so receivers can deduplicate.
- **`info` alerts never re-notify**; `warning`/`critical` re-notify every `renotify_s` until acknowledged.

## Consequences

- **Pro:** no broker, no lost notifications, restart-safe; the queue is inspectable with SQL.
- **Pro:** silences and routing are evaluated once, so what was suppressed is recorded and auditable.
- **Con:** a transaction stays open during each send (bounded by the timeout); fine at self-hosted scale, a lease
  column would replace it if jobs pile up.
- **Con:** duplicates are possible after a crash; exactly-once is not offered.
- Rules out: sending directly from the poll cycle, and in-memory queues.
