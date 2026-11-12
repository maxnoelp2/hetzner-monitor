-- M3b: inventory, hygiene and cost checks, traffic forecast.
-- See docs/design/checks.md, docs/design/data-model.md and ADR-0011.

ALTER TABLE rules DROP CONSTRAINT rules_check_type_check;
ALTER TABLE rules ADD CONSTRAINT rules_check_type_check
    CHECK (check_type IN ('server_status', 'resource_lifecycle', 'lb_target_health', 'traffic_quota',
                          'hetzner_api', 'cpu', 'network', 'disk_io', 'certificate_expiry', 'deprecation',
                          'hygiene_backups', 'hygiene_protection', 'hygiene_orphans', 'cost_change'));

-- Estimated monthly cost of a project, one row per UTC day (the day's latest inventory cycle).
-- A derived value Hetzner does not keep (ADR-0011); rows older than 90 days are deleted.
CREATE TABLE cost_estimates (
    project_id  uuid             NOT NULL REFERENCES hetzner_projects (id) ON DELETE CASCADE,
    day         date             NOT NULL,
    monthly_net double precision NOT NULL CHECK (monthly_net >= 0),
    currency    text             NOT NULL CHECK (length(currency) BETWEEN 1 AND 10),
    -- Resources that had no price in /pricing (not included in monthly_net).
    unpriced    integer          NOT NULL DEFAULT 0 CHECK (unpriced >= 0),
    at          timestamptz      NOT NULL,
    PRIMARY KEY (project_id, day)
);

-- First outgoing-traffic counter seen per server / load balancer and UTC day: the baseline of
-- the traffic forecast (ADR-0011). Rows older than 40 days are deleted.
CREATE TABLE traffic_samples (
    resource_id      uuid        NOT NULL REFERENCES resources (id) ON DELETE CASCADE,
    day              date        NOT NULL,
    at               timestamptz NOT NULL,
    outgoing_traffic bigint      NOT NULL CHECK (outgoing_traffic >= 0),
    PRIMARY KEY (resource_id, day)
);

-- Default rules of a project (docs/design/checks.md) — replaces the version from 0004 and stays
-- the single source of the defaults.
CREATE OR REPLACE FUNCTION hm_create_default_rules(p_org uuid, p_project uuid) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO rules (org_id, project_id, name, check_type, params, severity, is_default)
    SELECT p.org_id, p.id, d.name, d.check_type, d.params::jsonb, d.severity, true
    FROM hetzner_projects p
    CROSS JOIN (VALUES
        ('Server not running',                     'server_status',      '{"for_s": 120, "transitional_grace_s": 600}', 'critical'),
        ('Resource created or deleted',            'resource_lifecycle', '{}',                                          'info'),
        ('Load balancer target unhealthy',         'lb_target_health',   '{"for_s": 120}',                              'critical'),
        ('Traffic quota 80 % used',                'traffic_quota',      '{"threshold_pct": 80}',                       'warning'),
        ('Traffic quota exhausted',                'traffic_quota',      '{"threshold_pct": 100}',                      'critical'),
        ('Hetzner API unreachable',                'hetzner_api',        '{}',                                          'critical'),
        ('CPU high',                               'cpu',                '{"threshold_pct": 90, "for_s": 600}',         'warning'),
        ('Certificate expiring or renewal failed', 'certificate_expiry', '{"days": 14, "on_failure": true}',            'warning'),
        ('Certificate expires within 3 days',      'certificate_expiry', '{"days": 3, "on_failure": false}',            'critical'),
        ('Type deprecated',                        'deprecation',        '{}',                                          'info'),
        ('Server backups disabled',                'hygiene_backups',    '{"for_s": 3600}',                             'info'),
        ('Delete/rebuild protection off',          'hygiene_protection', '{"kinds": ["server"], "for_s": 3600}',        'info'),
        ('Unattached volume or IP',                'hygiene_orphans',    '{"for_s": 86400}',                            'info'),
        ('Estimated cost increased',               'cost_change',        '{"threshold_pct": 10}',                       'info'),
        ('Traffic quota forecast exceeded',        'traffic_quota',      '{"threshold_pct": 100, "forecast": true}',    'warning')
    ) AS d (name, check_type, params, severity)
    WHERE p.id = p_project AND p.org_id = p_org;
$$;

-- Existing projects get the new default rules.
INSERT INTO rules (org_id, project_id, name, check_type, params, severity, is_default)
SELECT p.org_id, p.id, d.name, d.check_type, d.params::jsonb, d.severity, true
FROM hetzner_projects p
CROSS JOIN (VALUES
    ('Certificate expiring or renewal failed', 'certificate_expiry', '{"days": 14, "on_failure": true}',         'warning'),
    ('Certificate expires within 3 days',      'certificate_expiry', '{"days": 3, "on_failure": false}',         'critical'),
    ('Type deprecated',                        'deprecation',        '{}',                                       'info'),
    ('Server backups disabled',                'hygiene_backups',    '{"for_s": 3600}',                          'info'),
    ('Delete/rebuild protection off',          'hygiene_protection', '{"kinds": ["server"], "for_s": 3600}',     'info'),
    ('Unattached volume or IP',                'hygiene_orphans',    '{"for_s": 86400}',                         'info'),
    ('Estimated cost increased',               'cost_change',        '{"threshold_pct": 10}',                    'info'),
    ('Traffic quota forecast exceeded',        'traffic_quota',      '{"threshold_pct": 100, "forecast": true}', 'warning')
) AS d (name, check_type, params, severity);
