-- M6b: Web Push (ADR-0017, docs/design/alerting.md#web-push).

-- The installation's VAPID key pair: one row, generated on first use. The private key is sealed
-- (AAD = row id) like every other secret; the public key is not secret.
CREATE TABLE vapid_keys (
    id             uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    singleton      boolean     NOT NULL DEFAULT true UNIQUE CHECK (singleton),
    private_ct     bytea       NOT NULL,
    private_dek_ct bytea       NOT NULL,
    key_version    integer     NOT NULL,
    public_key     text        NOT NULL,
    created_at     timestamptz NOT NULL DEFAULT now()
);

-- Push subscriptions, one per user and device (browser). Endpoint and keys are a secret: sealed
-- (AAD = row id). `endpoint_hash` (SHA-256 of the endpoint, hex) makes registration idempotent.
-- A removed or expired subscription keeps its row as a tombstone (`deleted_at`, ciphertext
-- wiped) so the delivery history of its jobs stays visible; tombstones are purged with the jobs.
CREATE TABLE push_subscriptions (
    id                  uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id             uuid        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    endpoint_hash       text        NOT NULL,
    subscription_ct     bytea       NOT NULL,
    subscription_dek_ct bytea       NOT NULL,
    key_version         integer     NOT NULL,
    name                text        NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    host                text        NOT NULL DEFAULT '',
    created_at          timestamptz NOT NULL,
    updated_at          timestamptz NOT NULL,
    deleted_at          timestamptz
);
CREATE UNIQUE INDEX push_subscriptions_endpoint_idx ON push_subscriptions (org_id, endpoint_hash)
    WHERE deleted_at IS NULL;
CREATE INDEX push_subscriptions_org_user_idx ON push_subscriptions (org_id, user_id) WHERE deleted_at IS NULL;
CREATE INDEX push_subscriptions_deleted_idx ON push_subscriptions (deleted_at) WHERE deleted_at IS NOT NULL;

-- New channel type. `push_users` copies the (non-secret) `users` of a web_push config so the
-- fan-out at enqueue time can use it without decrypting; NULL = every member of the org.
ALTER TABLE notification_channels DROP CONSTRAINT notification_channels_type_check;
ALTER TABLE notification_channels ADD CONSTRAINT notification_channels_type_check
    CHECK (type IN ('email', 'slack', 'discord', 'webhook', 'ntfy', 'web_push'));
ALTER TABLE notification_channels ADD COLUMN push_users uuid[];

-- A web_push channel fans out into one job per subscription (ADR-0017); other jobs have none.
ALTER TABLE notification_jobs ADD COLUMN push_subscription_id uuid
    REFERENCES push_subscriptions (id) ON DELETE CASCADE;
ALTER TABLE notification_jobs DROP CONSTRAINT notification_jobs_alert_event_id_channel_id_key;
CREATE UNIQUE INDEX notification_jobs_event_target_idx
    ON notification_jobs (alert_event_id, channel_id, push_subscription_id) NULLS NOT DISTINCT;
CREATE INDEX notification_jobs_push_subscription_id_idx ON notification_jobs (push_subscription_id)
    WHERE push_subscription_id IS NOT NULL;
