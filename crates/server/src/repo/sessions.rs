//! Server-side sessions (ADR-0013). Only token hashes are stored.

use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::{OrgId, UserId};

/// Absolute session lifetime.
pub const MAX_AGE_DAYS: i32 = 30;
/// A session unused for this long ends.
pub const IDLE_DAYS: i32 = 14;

/// A valid session as needed for authentication.
#[derive(Debug, sqlx::FromRow)]
pub struct SessionForAuth {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
}

/// Pre-authentication lookup by token hash: only live sessions of enabled users that still
/// hold the membership. Returns the tenant.
pub async fn find_valid(
    db: &PgPool,
    token_hash: &[u8],
) -> Result<Option<SessionForAuth>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.org_id, s.user_id, m.role
         FROM sessions s
         JOIN users u ON u.id = s.user_id AND u.disabled_at IS NULL
         JOIN memberships m ON m.org_id = s.org_id AND m.user_id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > now()
           AND s.last_seen_at > now() - make_interval(days => $2)",
    )
    .bind(token_hash)
    .bind(IDLE_DAYS)
    .fetch_optional(db)
    .await
}

/// Slides the idle timeout, at most once per minute.
pub async fn touch(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE sessions SET last_seen_at = now()
         WHERE id = $1 AND last_seen_at < now() - interval '1 minute'",
    )
    .bind(id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn insert(
    db: impl PgExecutor<'_>,
    org: OrgId,
    user: UserId,
    token_hash: &[u8],
    user_agent: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO sessions (org_id, user_id, token_hash, user_agent, expires_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(days => $5)) RETURNING id",
    )
    .bind(org.0)
    .bind(user.0)
    .bind(token_hash)
    .bind(user_agent)
    .bind(MAX_AGE_DAYS)
    .fetch_one(db)
    .await
}

/// Ends one session of this org.
pub async fn delete(db: &PgPool, org: OrgId, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sessions WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org.0)
        .execute(db)
        .await?;
    Ok(())
}

/// Ends all sessions of a user in every org (password change or reset), except `keep`.
/// Scoped by user, not org: a password belongs to the user.
pub async fn delete_for_user(
    db: impl PgExecutor<'_>,
    user: UserId,
    keep: Option<Uuid>,
) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND id IS DISTINCT FROM $2")
            .bind(user.0)
            .bind(keep)
            .execute(db)
            .await?
            .rows_affected(),
    )
}

/// Removes expired and idle sessions of all tenants (housekeeping, run on login).
pub async fn purge_expired(db: &PgPool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM sessions
         WHERE expires_at <= now() OR last_seen_at <= now() - make_interval(days => $1)",
    )
    .bind(IDLE_DAYS)
    .execute(db)
    .await?
    .rows_affected())
}
