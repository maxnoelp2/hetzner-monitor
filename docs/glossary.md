# Glossary

| Term | Meaning |
|---|---|
| **Organisation (org)** | Tenant. Owns users (via memberships), projects, rules, channels, silences. |
| **Project** | A connected Hetzner Cloud project = one API token. Hetzner tokens are scoped to exactly one project. |
| **Resource** | Something discovered in a project: server, load balancer, volume, certificate, floating IP, primary IP. |
| **Snapshot** | Last stored API representation of a resource (`resources.snapshot`). |
| **Inventory diff** | Comparison of two snapshots to detect created/deleted resources (Hetzner removed project-wide action listing on 2025-01-30). |
| **Check** | A typed evaluator (`check_type`) that turns a snapshot and/or metric series into a value and a condition. |
| **Rule** | A configured check: params (threshold, duration), scope, severity, channels. |
| **Scope** | Which resources a rule applies to: whole project, one resource, or a Hetzner label selector. |
| **Alert** | The state of one (rule, resource) pair: `pending`, `firing`, `resolved`. |
| **Alert event** | Immutable log entry of an alert transition (`pending`, `fired`, `resolved`, `acked`, `renotified`). |
| **Silence** | Time window + scope during which alerts still evaluate but do not notify (maintenance window). |
| **Channel** | A notification destination (email, Slack, Discord, webhook, ntfy, web push). |
| **Tier** | Polling layer with its own cadence: **fast** (status/traffic), **metrics** (time series), **inventory** (volumes, certs, pricing…). |
| **Budget** | Share of a project's Hetzner rate limit the tool may use (default 50 % of `RateLimit-Limit`). |
| **Degraded** | Project state when the Hetzner API is failing for it; availability alerts are suspended. |
| **DEK** | Data encryption key, random per secret-bearing row, encrypts the secret. |
| **Master key** | Instance key (`HM_MASTER_KEY`) that encrypts DEKs. Versioned for rotation. |
| **Envelope encryption** | Secret encrypted with a DEK, DEK encrypted with the master key. |
