use hm_core::apikey::NewApiKey;
use sqlx::{PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{OrgId, UserId};
use crate::auth::Scope;

/// Key row as needed for authentication.
#[derive(Debug, sqlx::FromRow)]
pub struct KeyForAuth {
    pub id: Uuid,
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub key_hash: Vec<u8>,
    pub scopes: Vec<String>,
}

/// Key metadata shown in the API (never the key or its hash).
#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct ApiKeyView {
    pub id: Uuid,
    pub name: String,
    /// First characters of the key, to recognise it.
    pub prefix: String,
    #[schema(value_type = Vec<crate::openapi::enums::Scope>)]
    pub scopes: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

/// Pre-authentication lookup: active (not revoked, not expired) key by prefix.
pub async fn find_active_by_prefix(
    db: &PgPool,
    prefix: &str,
) -> Result<Option<KeyForAuth>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, org_id, user_id, key_hash, scopes FROM api_keys
         WHERE prefix = $1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now())",
    )
    .bind(prefix)
    .fetch_optional(db)
    .await
}

/// Updates `last_used_at` at most once per minute.
pub async fn touch(db: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE api_keys SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')",
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
    name: &str,
    scopes: &[Scope],
    key: &NewApiKey,
) -> Result<ApiKeyView, sqlx::Error> {
    let scopes: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
    sqlx::query_as(
        "INSERT INTO api_keys (org_id, user_id, name, prefix, key_hash, scopes)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, name, prefix, scopes, created_at, last_used_at, revoked_at",
    )
    .bind(org.0)
    .bind(user.0)
    .bind(name)
    .bind(&key.prefix)
    .bind(&key.hash)
    .bind(&scopes)
    .fetch_one(db)
    .await
}

pub async fn list(db: &PgPool, org: OrgId) -> Result<Vec<ApiKeyView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, prefix, scopes, created_at, last_used_at, revoked_at
         FROM api_keys WHERE org_id = $1 ORDER BY created_at",
    )
    .bind(org.0)
    .fetch_all(db)
    .await
}

/// Revokes a key of this org. Returns `false` if it does not exist (or belongs to another org).
pub async fn revoke(db: &PgPool, org: OrgId, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at = coalesce(revoked_at, now()) WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .execute(db)
    .await?;
    Ok(result.rows_affected() == 1)
}
