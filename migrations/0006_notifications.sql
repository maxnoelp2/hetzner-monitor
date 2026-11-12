-- M4: notification channels, routing, the delivery queue and silences (docs/design/alerting.md, ADR-0012).

-- Channel configs hold webhook URLs, tokens, signing secrets and recipients: always envelope-encrypted
-- (AAD = channel id, docs/design/security.md). `hint` is a non-secret description (URL host, recipient count).
CREATE TABLE notification_channels (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id        uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    type          text        NOT NULL CHECK (type IN ('email', 'slack', 'discord', 'webhook', 'ntfy')),
    name          text        NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    config_ct     bytea       NOT NULL,
    config_dek_ct bytea       NOT NULL,
    key_version   integer     NOT NULL,
    hint          text        NOT NULL DEFAULT '',
    min_severity  text        NOT NULL DEFAULT 'warning' CHECK (min_severity IN ('info', 'warning', 'critical')),
    enabled       boolean     NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX notification_channels_org_name_idx ON notification_channels (org_id, lower(name));

-- Channels pinned to a rule. A rule with pins notifies only those channels.
CREATE TABLE rule_channels (
    rule_id    uuid NOT NULL REFERENCES rules (id) ON DELETE CASCADE,
    channel_id uuid NOT NULL REFERENCES notification_channels (id) ON DELETE CASCADE,
    PRIMARY KEY (rule_id, channel_id)
);
CREATE INDEX rule_channels_channel_id_idx ON rule_channels (channel_id);

-- Durable delivery queue (invariant 9). One row per (alert event, channel).
CREATE TABLE notification_jobs (
    id              bigserial   PRIMARY KEY,
    alert_event_id  bigint      NOT NULL REFERENCES alert_events (id) ON DELETE CASCADE,
    channel_id      uuid        NOT NULL REFERENCES notification_channels (id) ON DELETE CASCADE,
    status          text        NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'sent', 'failed')),
    attempts        integer     NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at timestamptz NOT NULL,
    last_error      text,
    created_at      timestamptz NOT NULL,
    finished_at     timestamptz,
    UNIQUE (alert_event_id, channel_id)
);
CREATE INDEX notification_jobs_due_idx ON notification_jobs (next_attempt_at) WHERE status = 'queued';
CREATE INDEX notification_jobs_channel_id_idx ON notification_jobs (channel_id);
CREATE INDEX notification_jobs_finished_idx ON notification_jobs (finished_at) WHERE status <> 'queued';

-- Silences: a time window plus a scope. All scope columns NULL = the whole org.
CREATE TABLE silences (
    id          uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id      uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    project_id  uuid        REFERENCES hetzner_projects (id) ON DELETE CASCADE,
    resource_id uuid        REFERENCES resources (id) ON DELETE CASCADE,
    rule_id     uuid        REFERENCES rules (id) ON DELETE CASCADE,
    selector    text,
    starts_at   timestamptz NOT NULL,
    ends_at     timestamptz NOT NULL,
    reason      text        NOT NULL DEFAULT '',
    created_by  uuid        REFERENCES users (id) ON DELETE SET NULL,
    created_at  timestamptz NOT NULL DEFAULT now(),
    CHECK (ends_at > starts_at)
);
CREATE INDEX silences_org_window_idx ON silences (org_id, ends_at);
