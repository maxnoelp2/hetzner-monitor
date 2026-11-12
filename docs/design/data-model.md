# Design: Data model (PostgreSQL)

Status: tenancy, API keys, projects, resources, rules, alerts, alert events, cost estimates, traffic samples, channels, rule channels, notification jobs, silences **implemented** (`migrations/0001`–`0006`); sessions, push subscriptions **proposed**.
Source of truth for implemented tables: `migrations/`.

🔒 = envelope-encrypted ([security](security.md)). All timestamps are `timestamptz` (UTC).

```sql
-- tenancy & auth
organizations(id uuid pk, name text, created_at)
users(id uuid pk, email citext unique, name text, password_hash text /* argon2id */, created_at, disabled_at)
memberships(org_id fk, user_id fk, role text /* owner|admin|member */, pk(org_id, user_id))
api_keys(id uuid pk, org_id fk, user_id fk, name text, prefix text unique /* 12 [a-z0-9] */, key_hash bytea /* sha256 */,
         scopes text[], last_used_at, expires_at, revoked_at, created_at)
sessions(id uuid pk, user_id fk, token_hash text, user_agent text, created_at, expires_at)

-- hetzner
hetzner_projects(
  id uuid pk /* app-generated: AAD of the token */, org_id fk, name text unique per org,
  token_ct bytea 🔒, token_dek_ct bytea 🔒, key_version int, token_hint text /* last 4 chars */,
  status text /* active|invalid_token|degraded|paused */, status_message text,
  budget_share real default 0.5,
  rl_limit int, rl_remaining int, rl_reset_at,
  metrics_interval_s int, last_fast_at, last_metrics_at, last_inventory_at,
  next_fast_at, next_inventory_at, next_metrics_at /* null = no metric rule in scope */,
  lease_until /* scheduler lease */, failures int /* consecutive */,
  budget_h int, fixed_h int,
  created_at)

resources(
  id uuid pk, project_id fk,
  kind text /* server|load_balancer|volume|certificate|floating_ip|primary_ip */,
  hetzner_id bigint, name text, labels jsonb, snapshot jsonb,
  first_seen_at, last_seen_at, deleted_at,
  unique(project_id, kind, hetzner_id))

-- derived daily snapshots (ADR-0011), written by inventory cycles
cost_estimates(project_id fk, day date /* UTC */, monthly_net double precision, currency text, unpriced int, at,
               pk(project_id, day))                  -- latest estimate of the day; 90 days kept
traffic_samples(resource_id fk, day date /* UTC */, at, outgoing_traffic bigint,
                pk(resource_id, day))                -- first counter of the day (servers, LBs); 40 days kept

-- alerting
rules(
  id uuid pk, org_id fk, project_id fk null /* null = all projects of the org */,
  name text, check_type text /* server_status|resource_lifecycle|lb_target_health|traffic_quota|hetzner_api|cpu|network|disk_io|
                                  certificate_expiry|deprecation|hygiene_backups|hygiene_protection|hygiene_orphans|cost_change */,
  params jsonb, selector text null, resource_id fk null,
  severity text /* info|warning|critical */, enabled bool, is_default bool,
  renotify_s int default 14400, created_at, updated_at)

alerts(
  id uuid pk, rule_id fk, resource_id fk null, project_id fk,
  state text /* pending|firing|resolved */, severity text, value double precision, message text,
  ok_cycles int /* flapping guard */, pending_since, fired_at, resolved_at, last_notified_at,
  acked_by fk null, acked_at, updated_at)
  -- partial unique: (rule_id, coalesce(resource_id, nil-uuid)) where state <> 'resolved'

alert_events(id bigserial pk, alert_id fk, type text /* pending|fired|resolved|acked|renotified|suppressed */,
             at, data jsonb)

silences(id uuid pk, org_id fk, project_id fk null, resource_id fk null, rule_id fk null, selector text null,
         starts_at, ends_at /* > starts_at */, reason text, created_by fk null, created_at)

-- notifications
notification_channels(id uuid pk, org_id fk, type text /* email|slack|discord|webhook|ntfy */, name text /* unique per org, case-insensitive */,
                      config_ct bytea 🔒, config_dek_ct bytea 🔒, key_version int, hint text /* non-secret */,
                      min_severity text default 'warning', enabled bool, created_at, updated_at)
rule_channels(rule_id fk, channel_id fk, pk(rule_id, channel_id))
notification_jobs(id bigserial pk, alert_event_id fk, channel_id fk, unique(alert_event_id, channel_id),
                  status text /* queued|sent|failed */, attempts int, next_attempt_at, last_error text /* redacted */,
                  created_at, finished_at)   -- finished jobs kept 90 days
push_subscriptions(id uuid pk, user_id fk, endpoint text, p256dh text, auth text, created_at)
```

## Notes

- **Tenancy:** every tenant table reaches `organizations` via `org_id` or a parent FK. Repositories take an `OrgId` argument — never query tenant data without it.
- **Bootstrap (self-hosted):** `hetzner-monitor bootstrap --org … --email …` creates the first org, an `owner` and an `admin` API key (printed once). No password until login lands (M5).
- **Resources** have no `org_id`; they are tenant-scoped through `hetzner_projects.org_id` (every query joins it).
- API keys are revoked by setting `revoked_at` (kept for audit), not deleted.
- **Retention:** finished `notification_jobs` 90 days (hourly cleanup in the notifier); `alert_events` retention is still proposed.
- **No metric tables** (invariant 8). `cost_estimates` and `traffic_samples` hold derived daily readings Hetzner does not keep
  ([ADR-0011](../adr/0011-derived-daily-snapshots.md)); they are tenant-scoped through their project (`traffic_samples` via `resources`).
- Enum-like `text` columns get `CHECK` constraints in migrations.
