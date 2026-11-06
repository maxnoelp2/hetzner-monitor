use hm_core::crypto::Sealed;
use hm_hcloud::RateLimit;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use super::OrgId;

/// Project as shown in the API. Contains no secret material.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ProjectView {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub status_message: Option<String>,
    pub token_hint: String,
    pub rl_limit: Option<i32>,
    pub rl_remaining: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub rl_reset_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_inventory_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Columns of [`ProjectView`]. A macro so queries stay `&'static str` (sqlx rejects dynamic SQL).
macro_rules! view_columns {
    () => {
        "id, name, status, status_message, token_hint, rl_limit, rl_remaining, rl_reset_at, last_inventory_at, created_at"
    };
}

/// Outcome of `insert`.
pub enum Inserted {
    Ok(ProjectView),
    NameTaken,
}

pub async fn insert(
    db: &PgPool,
    org: OrgId,
    id: Uuid,
    name: &str,
    sealed: &Sealed,
    token_hint: &str,
) -> Result<Inserted, sqlx::Error> {
    let result = sqlx::query_as(concat!(
        "INSERT INTO hetzner_projects (id, org_id, name, token_ct, token_dek_ct, key_version, token_hint)
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING ",
        view_columns!()
    ))
    .bind(id)
    .bind(org.0)
    .bind(name)
    .bind(&sealed.secret_ct)
    .bind(&sealed.dek_ct)
    .bind(sealed.key_version)
    .bind(token_hint)
    .fetch_one(db)
    .await;
    match result {
        Ok(view) => Ok(Inserted::Ok(view)),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Ok(Inserted::NameTaken),
        Err(err) => Err(err),
    }
}

pub async fn list(db: &PgPool, org: OrgId) -> Result<Vec<ProjectView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        view_columns!(),
        " FROM hetzner_projects WHERE org_id = $1 ORDER BY name"
    ))
    .bind(org.0)
    .fetch_all(db)
    .await
}

pub async fn get(db: &PgPool, org: OrgId, id: Uuid) -> Result<Option<ProjectView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        view_columns!(),
        " FROM hetzner_projects WHERE id = $1 AND org_id = $2"
    ))
    .bind(id)
    .bind(org.0)
    .fetch_optional(db)
    .await
}

pub async fn delete(db: &PgPool, org: OrgId, id: Uuid) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM hetzner_projects WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org.0)
        .execute(db)
        .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn sealed_token(
    db: &PgPool,
    org: OrgId,
    id: Uuid,
) -> Result<Option<Sealed>, sqlx::Error> {
    let row: Option<(Vec<u8>, Vec<u8>, i32)> = sqlx::query_as(
        "SELECT token_ct, token_dek_ct, key_version FROM hetzner_projects WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(secret_ct, dek_ct, key_version)| Sealed {
        secret_ct,
        dek_ct,
        key_version,
    }))
}

/// Records the project's health after talking to Hetzner.
pub async fn set_status(
    db: impl sqlx::PgExecutor<'_>,
    org: OrgId,
    id: Uuid,
    status: &str,
    message: Option<&str>,
    rate_limit: Option<RateLimit>,
    inventory_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    let rl = rate_limit.unwrap_or_default();
    let reset = rl
        .reset_at
        .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok());
    sqlx::query(
        "UPDATE hetzner_projects SET status = $3, status_message = $4,
             rl_limit = coalesce($5, rl_limit), rl_remaining = coalesce($6, rl_remaining),
             rl_reset_at = coalesce($7, rl_reset_at), last_inventory_at = coalesce($8, last_inventory_at)
         WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .bind(status)
    .bind(message)
    .bind(rl.limit.and_then(|v| i32::try_from(v).ok()))
    .bind(rl.remaining.and_then(|v| i32::try_from(v).ok()))
    .bind(reset)
    .bind(inventory_at)
    .execute(db)
    .await?;
    Ok(())
}
