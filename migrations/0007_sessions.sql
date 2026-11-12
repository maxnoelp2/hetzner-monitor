-- 0007: server-side sessions for password login (ADR-0007, ADR-0013). See docs/design/data-model.md.

CREATE TABLE sessions (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       uuid        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    org_id        uuid        NOT NULL,
    -- SHA-256 of the cookie token; the token is never stored. The CSRF token is derived from it.
    token_hash    bytea       NOT NULL UNIQUE CHECK (length(token_hash) = 32),
    user_agent    text        CHECK (length(user_agent) <= 300),
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    -- Absolute end of the session; the idle timeout is checked against last_seen_at.
    expires_at    timestamptz NOT NULL,
    -- Losing the membership ends the session.
    FOREIGN KEY (org_id, user_id) REFERENCES memberships (org_id, user_id) ON DELETE CASCADE
);
CREATE INDEX sessions_user_id_idx ON sessions (user_id);
CREATE INDEX sessions_org_id_idx ON sessions (org_id);
CREATE INDEX sessions_expires_at_idx ON sessions (expires_at);
