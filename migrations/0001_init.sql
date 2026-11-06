-- 0001_init: tenancy core (ADR-0006). See docs/design/data-model.md.

CREATE EXTENSION IF NOT EXISTS citext;

CREATE TABLE organizations (
    id          uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        text        NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id             uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    email          citext      NOT NULL UNIQUE,
    name           text        NOT NULL DEFAULT '',
    -- Argon2id PHC string. NULL once external identity providers exist (post-MVP).
    password_hash  text,
    created_at     timestamptz NOT NULL DEFAULT now(),
    disabled_at    timestamptz
);

CREATE TABLE memberships (
    org_id      uuid        NOT NULL REFERENCES organizations (id) ON DELETE CASCADE,
    user_id     uuid        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role        text        NOT NULL CHECK (role IN ('owner', 'admin', 'member')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

CREATE INDEX memberships_user_id_idx ON memberships (user_id);
