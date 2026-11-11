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
    pub last_fast_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_inventory_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_fast_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_inventory_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_metrics_at: Option<OffsetDateTime>,
    /// `null` = no enabled metric rule covers a server of this project; the metrics tier is idle.
    #[serde(with = "time::serde::rfc3339::option")]
    pub next_metrics_at: Option<OffsetDateTime>,
    /// Consecutive failed cycles.
    pub failures: i32,
    /// Share of `RateLimit-Limit` this tool may use.
    pub budget_share: f32,
    /// Requests per hour this tool may use (`rl_limit × budget_share`).
    pub budget_h: Option<i32>,
    /// Requests per hour of the fast and inventory tiers.
    pub fixed_h: Option<i32>,
    /// Metrics interval for the servers in scope of metric rules; `null` = no such server, or paused (budget used up).
    pub metrics_interval_s: Option<i32>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Columns of [`ProjectView`]. A macro so queries stay `&'static str` (sqlx rejects dynamic SQL).
macro_rules! view_columns {
    () => {
        "id, name, status, status_message, token_hint, rl_limit, rl_remaining, rl_reset_at, last_fast_at,
         last_inventory_at, next_fast_at, next_inventory_at, last_metrics_at, next_metrics_at, failures,
         budget_share, budget_h, fixed_h, metrics_interval_s, created_at"
    };
}

/// Outcome of `insert`.
pub enum Inserted {
    Ok(Box<ProjectView>),
    NameTaken,
}

pub async fn insert(
    db: impl sqlx::PgExecutor<'_>,
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
        Ok(view) => Ok(Inserted::Ok(Box::new(view))),
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

/// A project taken over for one poll cycle (scheduler or manual sync), incl. its sealed token.
#[derive(Debug, sqlx::FromRow)]
pub struct Claimed {
    pub id: Uuid,
    pub org_id: Uuid,
    pub status: String,
    pub failures: i32,
    pub budget_share: f32,
    pub last_inventory_at: Option<OffsetDateTime>,
    pub next_fast_at: Option<OffsetDateTime>,
    pub next_inventory_at: Option<OffsetDateTime>,
    pub next_metrics_at: Option<OffsetDateTime>,
    pub budget_h: Option<i32>,
    pub fixed_h: Option<i32>,
    pub token_ct: Vec<u8>,
    pub token_dek_ct: Vec<u8>,
    pub key_version: i32,
}

impl Claimed {
    pub fn org(&self) -> OrgId {
        OrgId(self.org_id)
    }

    pub fn sealed(&self) -> Sealed {
        Sealed {
            secret_ct: self.token_ct.clone(),
            dek_ct: self.token_dek_ct.clone(),
            key_version: self.key_version,
        }
    }
}

macro_rules! claimed_columns {
    () => {
        "id, org_id, status, failures, budget_share, last_inventory_at, next_fast_at, next_inventory_at,
         next_metrics_at, budget_h, fixed_h, token_ct, token_dek_ct, key_version"
    };
}

/// Claims up to `limit` projects whose fast or inventory cycle is due, across **all tenants**
/// (the scheduler is not acting for a tenant; every claimed row carries its `org_id`, and all
/// later queries use it). The row lock is held only for this statement; the claim itself is
/// the lease, which expires at `lease_until` if the worker dies.
pub async fn claim_due(
    db: &PgPool,
    now: OffsetDateTime,
    lease_until: OffsetDateTime,
    limit: i64,
) -> Result<Vec<Claimed>, sqlx::Error> {
    sqlx::query_as(concat!(
        "UPDATE hetzner_projects SET lease_until = $2
         WHERE id IN (
             SELECT id FROM hetzner_projects
             WHERE status IN ('active', 'degraded')
               AND (lease_until IS NULL OR lease_until < $1)
               AND (next_fast_at IS NULL OR next_fast_at <= $1
                    OR next_inventory_at IS NULL OR next_inventory_at <= $1
                    OR next_metrics_at <= $1)
             ORDER BY next_fast_at NULLS FIRST
             LIMIT $3
             FOR UPDATE SKIP LOCKED)
         RETURNING ",
        claimed_columns!()
    ))
    .bind(now)
    .bind(lease_until)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Takes the lease of one project of this org for a manual sync, regardless of its status.
/// `None` if the project does not exist or another cycle holds the lease.
pub async fn try_lease(
    db: &PgPool,
    org: OrgId,
    id: Uuid,
    now: OffsetDateTime,
    lease_until: OffsetDateTime,
) -> Result<Option<Claimed>, sqlx::Error> {
    sqlx::query_as(concat!(
        "UPDATE hetzner_projects SET lease_until = $4
         WHERE id = $1 AND org_id = $2 AND (lease_until IS NULL OR lease_until < $3)
         RETURNING ",
        claimed_columns!()
    ))
    .bind(id)
    .bind(org.0)
    .bind(now)
    .bind(lease_until)
    .fetch_optional(db)
    .await
}

/// True if a project of this org exists (used to tell "busy" from "not found").
pub async fn exists(db: &PgPool, org: OrgId, id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM hetzner_projects WHERE id = $1 AND org_id = $2)",
    )
    .bind(id)
    .bind(org.0)
    .fetch_one(db)
    .await
}

/// Outcome of a successful cycle.
pub struct Success {
    pub rate_limit: RateLimit,
    pub inventory: bool,
    pub next_fast_at: OffsetDateTime,
    pub next_inventory_at: Option<OffsetDateTime>,
    pub budget_h: u32,
    pub fixed_h: u32,
    pub metrics_interval_s: Option<u32>,
    /// `None`: no metric rule in scope, the metrics tier stops. `Some(t)`: the metrics tier runs;
    /// it starts at `t` if it was idle, an existing schedule is kept.
    pub metrics_from: Option<OffsetDateTime>,
}

/// Records a successful fast or inventory cycle: project `active`, failures reset, schedule
/// and budget updated. The caller releases the lease.
pub async fn record_success(
    conn: &mut sqlx::PgConnection,
    org: OrgId,
    id: Uuid,
    now: OffsetDateTime,
    s: &Success,
) -> Result<(), sqlx::Error> {
    let (limit, remaining, reset) = rate_limit_columns(s.rate_limit);
    let int = |v: u32| i32::try_from(v).unwrap_or(i32::MAX);
    sqlx::query(
        "UPDATE hetzner_projects SET status = 'active', status_message = NULL, failures = 0,
             rl_limit = coalesce($3, rl_limit), rl_remaining = coalesce($4, rl_remaining),
             rl_reset_at = coalesce($5, rl_reset_at),
             last_fast_at = $6, last_inventory_at = CASE WHEN $7 THEN $6 ELSE last_inventory_at END,
             next_fast_at = $8, next_inventory_at = coalesce($9, next_inventory_at),
             budget_h = $10, fixed_h = $11, metrics_interval_s = $12,
             next_metrics_at = CASE WHEN $13::timestamptz IS NULL THEN NULL
                                    ELSE coalesce(next_metrics_at, $13) END
         WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .bind(limit)
    .bind(remaining)
    .bind(reset)
    .bind(now)
    .bind(s.inventory)
    .bind(s.next_fast_at)
    .bind(s.next_inventory_at)
    .bind(int(s.budget_h))
    .bind(int(s.fixed_h))
    .bind(s.metrics_interval_s.map(int))
    .bind(s.metrics_from)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Records a successful metrics cycle. `next_metrics_at = None` stops the metrics tier.
pub async fn record_metrics_success(
    conn: &mut sqlx::PgConnection,
    org: OrgId,
    id: Uuid,
    now: OffsetDateTime,
    rate_limit: RateLimit,
    metrics_interval_s: Option<u32>,
    next_metrics_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    let (limit, remaining, reset) = rate_limit_columns(rate_limit);
    sqlx::query(
        "UPDATE hetzner_projects SET status = 'active', status_message = NULL, failures = 0,
             rl_limit = coalesce($3, rl_limit), rl_remaining = coalesce($4, rl_remaining),
             rl_reset_at = coalesce($5, rl_reset_at),
             last_metrics_at = $6, metrics_interval_s = $7, next_metrics_at = $8
         WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .bind(limit)
    .bind(remaining)
    .bind(reset)
    .bind(now)
    .bind(metrics_interval_s.map(|v| i32::try_from(v).unwrap_or(i32::MAX)))
    .bind(next_metrics_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Outcome of a failed cycle.
pub struct Failure<'a> {
    pub status: &'a str,
    pub message: &'a str,
    pub failures: i32,
    pub rate_limit: RateLimit,
    /// No cycle of either tier before this time (backoff / rate-limit reset).
    pub retry_at: OffsetDateTime,
}

/// Records a failed cycle. All tiers wait until `retry_at`. The caller releases the lease.
pub async fn record_failure(
    conn: &mut sqlx::PgConnection,
    org: OrgId,
    id: Uuid,
    f: &Failure<'_>,
) -> Result<(), sqlx::Error> {
    let (limit, remaining, reset) = rate_limit_columns(f.rate_limit);
    sqlx::query(
        "UPDATE hetzner_projects SET status = $3, status_message = $4, failures = $5,
             rl_limit = coalesce($6, rl_limit), rl_remaining = coalesce($7, rl_remaining),
             rl_reset_at = coalesce($8, rl_reset_at),
             next_fast_at = $9, next_inventory_at = greatest(coalesce(next_inventory_at, $9), $9),
             next_metrics_at = CASE WHEN next_metrics_at IS NULL THEN NULL
                                    ELSE greatest(next_metrics_at, $9) END
         WHERE id = $1 AND org_id = $2",
    )
    .bind(id)
    .bind(org.0)
    .bind(f.status)
    .bind(f.message)
    .bind(f.failures)
    .bind(limit)
    .bind(remaining)
    .bind(reset)
    .bind(f.retry_at)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Releases the lease after a cycle (or several tiers in a row) of a claim.
pub async fn release(db: &PgPool, org: OrgId, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE hetzner_projects SET lease_until = NULL WHERE id = $1 AND org_id = $2")
        .bind(id)
        .bind(org.0)
        .execute(db)
        .await?;
    Ok(())
}

fn rate_limit_columns(rl: RateLimit) -> (Option<i32>, Option<i32>, Option<OffsetDateTime>) {
    (
        rl.limit.and_then(|v| i32::try_from(v).ok()),
        rl.remaining.and_then(|v| i32::try_from(v).ok()),
        rl.reset_at
            .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok()),
    )
}
