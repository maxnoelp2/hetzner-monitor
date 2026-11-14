//! Push subscriptions (ADR-0017): one per user and device, sealed (AAD = row id). Removed or
//! expired subscriptions stay as tombstones (`deleted_at` set, ciphertext wiped) so the
//! delivery history of their jobs stays visible; [`purge`] deletes old tombstones.

use hm_core::crypto::Sealed;
use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{OrgId, UserId};

/// A subscription as shown in the API: no endpoint, no keys.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct PushSubscriptionView {
    pub id: Uuid,
    /// The device name given when subscribing.
    pub name: String,
    /// The push service's host (`fcm.googleapis.com`, `web.push.apple.com`, …).
    pub host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

macro_rules! columns {
    () => {
        "id, name, host, created_at, updated_at"
    };
}

/// A live subscription found by its endpoint: id, owner and sealed data.
#[derive(Debug, sqlx::FromRow)]
pub struct Existing {
    pub id: Uuid,
    pub user_id: Uuid,
    pub subscription_ct: Vec<u8>,
    pub subscription_dek_ct: Vec<u8>,
    pub key_version: i32,
}

impl Existing {
    pub fn sealed(&self) -> Sealed {
        Sealed {
            secret_ct: self.subscription_ct.clone(),
            dek_ct: self.subscription_dek_ct.clone(),
            key_version: self.key_version,
        }
    }
}

/// The live subscription of this org with this endpoint hash, locked for an update.
pub async fn find_by_endpoint(
    conn: &mut PgConnection,
    org: OrgId,
    endpoint_hash: &str,
) -> Result<Option<Existing>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, user_id, subscription_ct, subscription_dek_ct, key_version FROM push_subscriptions
         WHERE org_id = $1 AND endpoint_hash = $2 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(org.0)
    .bind(endpoint_hash)
    .fetch_optional(&mut *conn)
    .await
}

pub struct NewSubscription<'a> {
    pub id: Uuid,
    pub endpoint_hash: &'a str,
    pub sealed: &'a Sealed,
    pub name: &'a str,
    pub host: &'a str,
}

/// Inserts a subscription; `None` if a live one with the same endpoint appeared meanwhile.
pub async fn insert(
    conn: &mut PgConnection,
    org: OrgId,
    user: UserId,
    s: &NewSubscription<'_>,
    now: OffsetDateTime,
) -> Result<Option<PushSubscriptionView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "INSERT INTO push_subscriptions (id, org_id, user_id, endpoint_hash, subscription_ct,
                                         subscription_dek_ct, key_version, name, host, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
         ON CONFLICT (org_id, endpoint_hash) WHERE deleted_at IS NULL DO NOTHING
         RETURNING ",
        columns!()
    ))
    .bind(s.id)
    .bind(org.0)
    .bind(user.0)
    .bind(s.endpoint_hash)
    .bind(&s.sealed.secret_ct)
    .bind(&s.sealed.dek_ct)
    .bind(s.sealed.key_version)
    .bind(s.name)
    .bind(s.host)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
}

/// Re-registers an existing endpoint (new keys, name, possibly another user of the org: the
/// browser is now theirs).
pub async fn replace(
    conn: &mut PgConnection,
    org: OrgId,
    user: UserId,
    s: &NewSubscription<'_>,
    now: OffsetDateTime,
) -> Result<PushSubscriptionView, sqlx::Error> {
    sqlx::query_as(concat!(
        "UPDATE push_subscriptions SET user_id = $3, subscription_ct = $4, subscription_dek_ct = $5,
             key_version = $6, name = $7, host = $8, updated_at = $9
         WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL
         RETURNING ",
        columns!()
    ))
    .bind(org.0)
    .bind(s.id)
    .bind(user.0)
    .bind(&s.sealed.secret_ct)
    .bind(&s.sealed.dek_ct)
    .bind(s.sealed.key_version)
    .bind(s.name)
    .bind(s.host)
    .bind(now)
    .fetch_one(&mut *conn)
    .await
}

/// The user's live subscriptions in this org.
pub async fn list(
    db: &PgPool,
    org: OrgId,
    user: UserId,
) -> Result<Vec<PushSubscriptionView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM push_subscriptions WHERE org_id = $1 AND user_id = $2 AND deleted_at IS NULL
          ORDER BY created_at, id"
    ))
    .bind(org.0)
    .bind(user.0)
    .fetch_all(db)
    .await
}

/// Removes one of the user's subscriptions (tombstone). `false` if there is none.
pub async fn delete(
    db: &PgPool,
    org: OrgId,
    user: UserId,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let done = sqlx::query(
        "UPDATE push_subscriptions SET deleted_at = $4, subscription_ct = '', subscription_dek_ct = ''
         WHERE org_id = $1 AND user_id = $2 AND id = $3 AND deleted_at IS NULL",
    )
    .bind(org.0)
    .bind(user.0)
    .bind(id)
    .bind(now)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Tombstones a subscription the push service reported gone (notifier; the id comes from a
/// claimed job of the same tenant).
pub async fn mark_gone(
    conn: &mut PgConnection,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE push_subscriptions SET deleted_at = $2, subscription_ct = '', subscription_dek_ct = ''
         WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(id)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A live subscription a channel targets, sealed.
#[derive(Debug, sqlx::FromRow)]
pub struct Target {
    pub id: Uuid,
    pub name: String,
    pub subscription_ct: Vec<u8>,
    pub subscription_dek_ct: Vec<u8>,
    pub key_version: i32,
}

impl Target {
    pub fn sealed(&self) -> Sealed {
        Sealed {
            secret_ct: self.subscription_ct.clone(),
            dek_ct: self.subscription_dek_ct.clone(),
            key_version: self.key_version,
        }
    }
}

/// The live subscriptions of `users` (`None`: every member) in this org, for a channel test.
pub async fn targets(
    db: impl PgExecutor<'_>,
    org: OrgId,
    users: Option<&[Uuid]>,
) -> Result<Vec<Target>, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.id, s.name, s.subscription_ct, s.subscription_dek_ct, s.key_version
         FROM push_subscriptions s
         JOIN memberships m ON m.org_id = s.org_id AND m.user_id = s.user_id
         JOIN users u ON u.id = s.user_id AND u.disabled_at IS NULL
         WHERE s.org_id = $1 AND s.deleted_at IS NULL AND ($2::uuid[] IS NULL OR s.user_id = ANY($2))
         ORDER BY s.created_at, s.id",
    )
    .bind(org.0)
    .bind(users)
    .fetch_all(db)
    .await
}

/// One of the user's live subscriptions in this org, sealed.
pub async fn own_target(
    db: &PgPool,
    org: OrgId,
    user: UserId,
    id: Uuid,
) -> Result<Option<Target>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, name, subscription_ct, subscription_dek_ct, key_version FROM push_subscriptions
         WHERE org_id = $1 AND user_id = $2 AND id = $3 AND deleted_at IS NULL",
    )
    .bind(org.0)
    .bind(user.0)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// How many of `users` are members of this org.
pub async fn count_members(
    db: impl PgExecutor<'_>,
    org: OrgId,
    users: &[Uuid],
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM memberships WHERE org_id = $1 AND user_id = ANY($2)")
        .bind(org.0)
        .bind(users)
        .fetch_one(db)
        .await
}

/// Deletes tombstones older than the job retention (their jobs are gone or go with them).
pub async fn purge(db: &PgPool, before: OffsetDateTime) -> Result<u64, sqlx::Error> {
    let done = sqlx::query("DELETE FROM push_subscriptions WHERE deleted_at < $1")
        .bind(before)
        .execute(db)
        .await?;
    Ok(done.rows_affected())
}
