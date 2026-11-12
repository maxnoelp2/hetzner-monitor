//! Silences: a time window plus a scope (project, resource, rule, label selector; all optional,
//! none = the whole org). Evaluation continues; covered notifications are not queued.

use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{OrgId, UserId};

#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct SilenceView {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub resource_id: Option<Uuid>,
    pub rule_id: Option<Uuid>,
    pub selector: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub starts_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub ends_at: OffsetDateTime,
    pub reason: String,
    pub created_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

macro_rules! columns {
    () => {
        "id, project_id, resource_id, rule_id, selector, starts_at, ends_at, reason, created_by, created_at"
    };
}

pub struct NewSilence<'a> {
    pub project_id: Option<Uuid>,
    pub resource_id: Option<Uuid>,
    pub rule_id: Option<Uuid>,
    pub selector: Option<&'a str>,
    pub starts_at: OffsetDateTime,
    pub ends_at: OffsetDateTime,
    pub reason: &'a str,
}

/// Outcome of [`insert`].
pub enum Inserted {
    Ok(Box<SilenceView>),
    /// `project_id`, `resource_id` or `rule_id` is not in the org.
    UnknownScope,
}

pub async fn insert(
    conn: &mut PgConnection,
    org: OrgId,
    user: UserId,
    s: &NewSilence<'_>,
    now: OffsetDateTime,
) -> Result<Inserted, sqlx::Error> {
    // Every given scope id must belong to the org (a resource via its project).
    let known: bool = sqlx::query_scalar(
        "SELECT ($2::uuid IS NULL OR EXISTS (SELECT 1 FROM hetzner_projects WHERE id = $2 AND org_id = $1))
            AND ($3::uuid IS NULL OR EXISTS (SELECT 1 FROM resources r JOIN hetzner_projects p ON p.id = r.project_id
                                             WHERE r.id = $3 AND p.org_id = $1))
            AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM rules WHERE id = $4 AND org_id = $1))",
    )
    .bind(org.0)
    .bind(s.project_id)
    .bind(s.resource_id)
    .bind(s.rule_id)
    .fetch_one(&mut *conn)
    .await?;
    if !known {
        return Ok(Inserted::UnknownScope);
    }
    let view = sqlx::query_as(concat!(
        "INSERT INTO silences (org_id, project_id, resource_id, rule_id, selector, starts_at, ends_at,
                               reason, created_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING ",
        columns!()
    ))
    .bind(org.0)
    .bind(s.project_id)
    .bind(s.resource_id)
    .bind(s.rule_id)
    .bind(s.selector)
    .bind(s.starts_at)
    .bind(s.ends_at)
    .bind(s.reason)
    .bind(user.0)
    .bind(now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(Inserted::Ok(Box::new(view)))
}

/// Silences of the org; `active_only` = not yet ended.
pub async fn list(
    db: &PgPool,
    org: OrgId,
    active_only: bool,
    now: OffsetDateTime,
) -> Result<Vec<SilenceView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM silences WHERE org_id = $1 AND (NOT $2 OR ends_at > $3) ORDER BY starts_at DESC, id"
    ))
    .bind(org.0)
    .bind(active_only)
    .bind(now)
    .fetch_all(db)
    .await
}

/// Ends a silence now: an active one gets `ends_at = now`, a future one is deleted, an ended
/// one stays as it is. Returns `false` if it is not in the org.
pub async fn end(
    db: &PgPool,
    org: OrgId,
    id: Uuid,
    now: OffsetDateTime,
) -> Result<bool, sqlx::Error> {
    let mut tx = db.begin().await?;
    let row: Option<(OffsetDateTime, OffsetDateTime)> = sqlx::query_as(
        "SELECT starts_at, ends_at FROM silences WHERE org_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(org.0)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((starts_at, ends_at)) = row else {
        return Ok(false);
    };
    if starts_at >= now {
        sqlx::query("DELETE FROM silences WHERE org_id = $1 AND id = $2")
            .bind(org.0)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    } else if ends_at > now {
        sqlx::query("UPDATE silences SET ends_at = $3 WHERE org_id = $1 AND id = $2")
            .bind(org.0)
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Active silences of the org whose id scope covers the alert: `(id, selector)`. The selector
/// is matched against the resource labels by the caller.
pub async fn covering(
    db: impl PgExecutor<'_>,
    org: OrgId,
    project_id: Uuid,
    resource_id: Option<Uuid>,
    rule_id: Uuid,
    now: OffsetDateTime,
) -> Result<Vec<(Uuid, Option<String>)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, selector FROM silences
         WHERE org_id = $1 AND starts_at <= $5 AND ends_at > $5
           AND (project_id IS NULL OR project_id = $2)
           AND (resource_id IS NULL OR resource_id = $3)
           AND (rule_id IS NULL OR rule_id = $4)
         ORDER BY created_at, id",
    )
    .bind(org.0)
    .bind(project_id)
    .bind(resource_id)
    .bind(rule_id)
    .bind(now)
    .fetch_all(db)
    .await
}
