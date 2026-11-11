-- M2: scheduling state on projects, rules, alerts and alert events.
-- See docs/design/scheduler.md, docs/design/alerting.md and docs/design/data-model.md.

-- Scheduling and budget state. The scheduler claims a due project by setting `lease_until`;
-- the row lock is held only for the claim, never across a Hetzner request.
ALTER TABLE hetzner_projects
    ADD COLUMN next_fast_at      timestamptz,
    ADD COLUMN next_inventory_at timestamptz,
    ADD COLUMN lease_until       timestamptz,
    ADD COLUMN failures          integer NOT NULL DEFAULT 0 CHECK (failures >= 0),
    ADD COLUMN budget_h          integer,
    ADD COLUMN fixed_h           integer;
CREATE INDEX hetzner_projects_due_idx ON hetzner_projects (next_fast_at)
    WHERE status IN ('active', 'degraded');

CREATE TABLE rules (
    id          uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    -- NULL = all projects of the org.
    project_id  uuid        REFERENCES hetzner_projects (id) ON DELETE CASCADE,
    name        text        NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    check_type  text        NOT NULL CHECK (check_type IN ('server_status', 'resource_lifecycle',
                                                           'lb_target_health', 'traffic_quota', 'hetzner_api')),
    params      jsonb       NOT NULL DEFAULT '{}',
    -- Hetzner label selector, e.g. `env=prod,role!=db`.
    selector    text        CHECK (length(selector) BETWEEN 1 AND 1000),
    resource_id uuid        REFERENCES resources (id) ON DELETE CASCADE,
    severity    text        NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    enabled     boolean     NOT NULL DEFAULT true,
    is_default  boolean     NOT NULL DEFAULT false,
    renotify_s  integer     NOT NULL DEFAULT 14400 CHECK (renotify_s >= 300),
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX rules_org_id_idx ON rules (org_id);
CREATE INDEX rules_project_id_idx ON rules (project_id);

CREATE TABLE alerts (
    id               uuid             PRIMARY KEY DEFAULT gen_random_uuid(),
    rule_id          uuid             NOT NULL REFERENCES rules (id) ON DELETE CASCADE,
    -- NULL for project-level alerts (e.g. `hetzner_api`).
    resource_id      uuid             REFERENCES resources (id) ON DELETE CASCADE,
    project_id       uuid             NOT NULL REFERENCES hetzner_projects (id) ON DELETE CASCADE,
    state            text             NOT NULL CHECK (state IN ('pending', 'firing', 'resolved')),
    severity         text             NOT NULL CHECK (severity IN ('info', 'warning', 'critical')),
    value            double precision,
    message          text             NOT NULL DEFAULT '',
    -- Consecutive good cycles while firing (flapping guard).
    ok_cycles        integer          NOT NULL DEFAULT 0 CHECK (ok_cycles >= 0),
    pending_since    timestamptz      NOT NULL,
    fired_at         timestamptz,
    resolved_at      timestamptz,
    last_notified_at timestamptz,
    acked_by         uuid             REFERENCES users (id) ON DELETE SET NULL,
    acked_at         timestamptz,
    updated_at       timestamptz      NOT NULL DEFAULT now()
);
-- Invariant 7: at most one open alert per (rule, resource).
CREATE UNIQUE INDEX alerts_one_open_idx
    ON alerts (rule_id, coalesce(resource_id, '00000000-0000-0000-0000-000000000000'::uuid))
    WHERE state <> 'resolved';
CREATE INDEX alerts_project_state_idx ON alerts (project_id, state);

CREATE TABLE alert_events (
    id       bigserial   PRIMARY KEY,
    alert_id uuid        NOT NULL REFERENCES alerts (id) ON DELETE CASCADE,
    type     text        NOT NULL CHECK (type IN ('pending', 'fired', 'resolved', 'acked', 'renotified', 'suppressed')),
    at       timestamptz NOT NULL,
    data     jsonb       NOT NULL DEFAULT '{}'
);
CREATE INDEX alert_events_alert_id_idx ON alert_events (alert_id, id);

-- Default rules of a project (docs/design/checks.md). The single source of the defaults:
-- used by this migration for existing projects and by the service on connect.
CREATE FUNCTION hm_create_default_rules(p_org uuid, p_project uuid) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO rules (org_id, project_id, name, check_type, params, severity, is_default)
    SELECT p.org_id, p.id, d.name, d.check_type, d.params::jsonb, d.severity, true
    FROM hetzner_projects p
    CROSS JOIN (VALUES
        ('Server not running',            'server_status',      '{"for_s": 120, "transitional_grace_s": 600}', 'critical'),
        ('Resource created or deleted',   'resource_lifecycle', '{}',                                          'info'),
        ('Load balancer target unhealthy', 'lb_target_health',  '{"for_s": 120}',                              'critical'),
        ('Traffic quota 80 % used',       'traffic_quota',      '{"threshold_pct": 80}',                       'warning'),
        ('Traffic quota exhausted',       'traffic_quota',      '{"threshold_pct": 100}',                      'critical'),
        ('Hetzner API unreachable',       'hetzner_api',        '{}',                                          'critical')
    ) AS d (name, check_type, params, severity)
    WHERE p.id = p_project AND p.org_id = p_org;
$$;

SELECT hm_create_default_rules(org_id, id) FROM hetzner_projects;
