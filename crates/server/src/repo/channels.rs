//! Notification channels and the channels pinned to rules. Configs are stored sealed
//! (AAD = channel id) and never leave this module unencrypted except via [`sealed_config`].

use hm_core::crypto::Sealed;
use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::OrgId;

/// A channel as shown in the API. No secret material.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct ChannelView {
    pub id: Uuid,
    #[serde(rename = "type")]
    #[schema(value_type = crate::openapi::enums::ChannelType)]
    pub kind: String,
    pub name: String,
    /// Non-secret description: target host or recipient count.
    pub hint: String,
    #[schema(value_type = crate::openapi::enums::Severity)]
    pub min_severity: String,
    pub enabled: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

macro_rules! columns {
    () => {
        "id, type AS kind, name, hint, min_severity, enabled, created_at, updated_at"
    };
}

pub struct NewChannel<'a> {
    pub id: Uuid,
    pub kind: &'a str,
    pub name: &'a str,
    pub sealed: &'a Sealed,
    pub hint: &'a str,
    pub min_severity: &'a str,
    pub enabled: bool,
}

pub async fn insert(
    db: impl PgExecutor<'_>,
    org: OrgId,
    c: &NewChannel<'_>,
    now: OffsetDateTime,
) -> Result<ChannelView, sqlx::Error> {
    sqlx::query_as(concat!(
        "INSERT INTO notification_channels (id, org_id, type, name, config_ct, config_dek_ct, key_version,
                                            hint, min_severity, enabled, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11)
         RETURNING ",
        columns!()
    ))
    .bind(c.id)
    .bind(org.0)
    .bind(c.kind)
    .bind(c.name)
    .bind(&c.sealed.secret_ct)
    .bind(&c.sealed.dek_ct)
    .bind(c.sealed.key_version)
    .bind(c.hint)
    .bind(c.min_severity)
    .bind(c.enabled)
    .bind(now)
    .fetch_one(db)
    .await
}

pub async fn list(db: &PgPool, org: OrgId) -> Result<Vec<ChannelView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM notification_channels WHERE org_id = $1 ORDER BY created_at, id"
    ))
    .bind(org.0)
    .fetch_all(db)
    .await
}

pub async fn get(
    db: impl PgExecutor<'_>,
    org: OrgId,
    id: Uuid,
) -> Result<Option<ChannelView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM notification_channels WHERE org_id = $1 AND id = $2"
    ))
    .bind(org.0)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The sealed config of a channel of this org.
pub async fn sealed_config(
    db: impl PgExecutor<'_>,
    org: OrgId,
    id: Uuid,
) -> Result<Option<Sealed>, sqlx::Error> {
    let row: Option<(Vec<u8>, Vec<u8>, i32)> = sqlx::query_as(
        "SELECT config_ct, config_dek_ct, key_version FROM notification_channels
         WHERE org_id = $1 AND id = $2",
    )
    .bind(org.0)
    .bind(id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(secret_ct, dek_ct, key_version)| Sealed {
        secret_ct,
        dek_ct,
        key_version,
    }))
}

/// Fields of `PATCH /channels/{id}`; `None` keeps the value.
#[derive(Default)]
pub struct Patch<'a> {
    pub name: Option<&'a str>,
    pub min_severity: Option<&'a str>,
    pub enabled: Option<bool>,
    /// A new config replaces the old one completely.
    pub config: Option<(&'a Sealed, &'a str)>,
}

pub async fn update(
    conn: &mut PgConnection,
    org: OrgId,
    id: Uuid,
    patch: &Patch<'_>,
    now: OffsetDateTime,
) -> Result<Option<ChannelView>, sqlx::Error> {
    let (ct, dek, version, hint) = match patch.config {
        Some((sealed, hint)) => (
            Some(sealed.secret_ct.as_slice()),
            Some(sealed.dek_ct.as_slice()),
            Some(sealed.key_version),
            Some(hint),
        ),
        None => (None, None, None, None),
    };
    sqlx::query_as(concat!(
        "UPDATE notification_channels SET
             name = coalesce($3, name),
             min_severity = coalesce($4, min_severity),
             enabled = coalesce($5, enabled),
             config_ct = coalesce($6, config_ct),
             config_dek_ct = coalesce($7, config_dek_ct),
             key_version = coalesce($8, key_version),
             hint = coalesce($9, hint),
             updated_at = $10
         WHERE org_id = $1 AND id = $2
         RETURNING ",
        columns!()
    ))
    .bind(org.0)
    .bind(id)
    .bind(patch.name)
    .bind(patch.min_severity)
    .bind(patch.enabled)
    .bind(ct)
    .bind(dek)
    .bind(version)
    .bind(hint)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
}

/// Deletes a channel of this org (its queued jobs and rule pins go with it).
pub async fn delete(db: &PgPool, org: OrgId, id: Uuid) -> Result<bool, sqlx::Error> {
    let done = sqlx::query("DELETE FROM notification_channels WHERE org_id = $1 AND id = $2")
        .bind(org.0)
        .bind(id)
        .execute(db)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Channel ids pinned to a rule of this org.
pub async fn for_rule(
    db: impl PgExecutor<'_>,
    org: OrgId,
    rule_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT rc.channel_id FROM rule_channels rc
         JOIN rules r ON r.id = rc.rule_id
         JOIN notification_channels c ON c.id = rc.channel_id AND c.org_id = r.org_id
         WHERE r.org_id = $1 AND rc.rule_id = $2 ORDER BY c.created_at, c.id",
    )
    .bind(org.0)
    .bind(rule_id)
    .fetch_all(db)
    .await
}

/// Replaces the channels pinned to a rule. Returns `false` (and changes nothing) if a
/// channel id does not belong to the org.
pub async fn set_for_rule(
    conn: &mut PgConnection,
    org: OrgId,
    rule_id: Uuid,
    channel_ids: &[Uuid],
) -> Result<bool, sqlx::Error> {
    let found: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT id) FROM notification_channels WHERE org_id = $1 AND id = ANY($2)",
    )
    .bind(org.0)
    .bind(channel_ids)
    .fetch_one(&mut *conn)
    .await?;
    let mut wanted = channel_ids.to_vec();
    wanted.sort_unstable();
    wanted.dedup();
    if usize::try_from(found).ok() != Some(wanted.len()) {
        return Ok(false);
    }
    sqlx::query(
        "DELETE FROM rule_channels rc USING rules r
         WHERE rc.rule_id = r.id AND r.org_id = $1 AND rc.rule_id = $2",
    )
    .bind(org.0)
    .bind(rule_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO rule_channels (rule_id, channel_id)
         SELECT r.id, c FROM rules r, unnest($3::uuid[]) AS c WHERE r.org_id = $1 AND r.id = $2",
    )
    .bind(org.0)
    .bind(rule_id)
    .bind(&wanted)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}
