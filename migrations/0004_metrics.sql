-- M3a: metrics tier and metric checks. See docs/design/scheduler.md and docs/design/checks.md.

-- NULL = no metric rule in scope, the metrics tier does not run for this project.
ALTER TABLE hetzner_projects ADD COLUMN next_metrics_at timestamptz;

ALTER TABLE rules DROP CONSTRAINT rules_check_type_check;
ALTER TABLE rules ADD CONSTRAINT rules_check_type_check
    CHECK (check_type IN ('server_status', 'resource_lifecycle', 'lb_target_health', 'traffic_quota',
                          'hetzner_api', 'cpu', 'network', 'disk_io'));

-- Default rules of a project (docs/design/checks.md) — replaces the version from 0003 and stays
-- the single source of the defaults. `network` and `disk_io` are templates without a default rule.
CREATE OR REPLACE FUNCTION hm_create_default_rules(p_org uuid, p_project uuid) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO rules (org_id, project_id, name, check_type, params, severity, is_default)
    SELECT p.org_id, p.id, d.name, d.check_type, d.params::jsonb, d.severity, true
    FROM hetzner_projects p
    CROSS JOIN (VALUES
        ('Server not running',             'server_status',      '{"for_s": 120, "transitional_grace_s": 600}', 'critical'),
        ('Resource created or deleted',    'resource_lifecycle', '{}',                                          'info'),
        ('Load balancer target unhealthy', 'lb_target_health',   '{"for_s": 120}',                              'critical'),
        ('Traffic quota 80 % used',        'traffic_quota',      '{"threshold_pct": 80}',                       'warning'),
        ('Traffic quota exhausted',        'traffic_quota',      '{"threshold_pct": 100}',                      'critical'),
        ('Hetzner API unreachable',        'hetzner_api',        '{}',                                          'critical'),
        ('CPU high',                       'cpu',                '{"threshold_pct": 90, "for_s": 600}',         'warning')
    ) AS d (name, check_type, params, severity)
    WHERE p.id = p_project AND p.org_id = p_org;
$$;

-- Existing projects get the new default rule.
INSERT INTO rules (org_id, project_id, name, check_type, params, severity, is_default)
SELECT org_id, id, 'CPU high', 'cpu', '{"threshold_pct": 90, "for_s": 600}'::jsonb, 'warning', true
FROM hetzner_projects;
