use serde_json::Value;
use sqlx::types::Json;
use sqlx::{PgConnection, PgExecutor, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::OrgId;

/// A rule as stored and shown in the API.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RuleView {
    pub id: Uuid,
    pub project_id: Option<Uuid>,
    pub name: String,
    pub check_type: String,
    pub params: Json<Value>,
    pub selector: Option<String>,
    pub resource_id: Option<Uuid>,
    pub severity: String,
    pub enabled: bool,
    pub is_default: bool,
    pub renotify_s: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

macro_rules! columns {
    () => {
        "id, project_id, name, check_type, params, selector, resource_id, severity, enabled, is_default,
         renotify_s, created_at, updated_at"
    };
}

/// Creates the default rules of a project (SQL function from migration 0003, the single
/// source of the defaults). Does nothing if the project is not in `org`.
pub async fn create_defaults(
    db: impl PgExecutor<'_>,
    org: OrgId,
    project_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT hm_create_default_rules($1, $2)")
        .bind(org.0)
        .bind(project_id)
        .execute(db)
        .await?;
    Ok(())
}

/// Enabled rules that apply to a project: its own and the org-wide ones.
pub async fn enabled_for_project(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
) -> Result<Vec<RuleView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM rules WHERE org_id = $1 AND (project_id = $2 OR project_id IS NULL) AND enabled
          ORDER BY created_at, id"
    ))
    .bind(org.0)
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await
}

/// Rules of the org, optionally only those of one project (incl. org-wide rules).
pub async fn list(
    db: &PgPool,
    org: OrgId,
    project_id: Option<Uuid>,
) -> Result<Vec<RuleView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM rules WHERE org_id = $1 AND ($2::uuid IS NULL OR project_id = $2 OR project_id IS NULL)
          ORDER BY project_id NULLS FIRST, created_at, id"
    ))
    .bind(org.0)
    .bind(project_id)
    .fetch_all(db)
    .await
}

pub async fn get(
    db: impl PgExecutor<'_>,
    org: OrgId,
    id: Uuid,
) -> Result<Option<RuleView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "SELECT ",
        columns!(),
        " FROM rules WHERE id = $1 AND org_id = $2"
    ))
    .bind(id)
    .bind(org.0)
    .fetch_optional(db)
    .await
}

/// Change of an optional field.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Change<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

/// Fields of a rule that can be changed. `None` = unchanged.
#[derive(Debug, Default)]
pub struct Patch {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub params: Option<Value>,
    pub severity: Option<String>,
    pub selector: Change<String>,
}

pub async fn update(
    conn: &mut PgConnection,
    org: OrgId,
    id: Uuid,
    patch: &Patch,
    now: OffsetDateTime,
) -> Result<Option<RuleView>, sqlx::Error> {
    sqlx::query_as(concat!(
        "UPDATE rules SET name = coalesce($3, name), enabled = coalesce($4, enabled),
             params = coalesce($5, params), severity = coalesce($6, severity),
             selector = CASE WHEN $7 THEN $8 ELSE selector END, updated_at = $9
         WHERE id = $1 AND org_id = $2 RETURNING ",
        columns!()
    ))
    .bind(id)
    .bind(org.0)
    .bind(patch.name.as_deref())
    .bind(patch.enabled)
    .bind(patch.params.as_ref().map(Json))
    .bind(patch.severity.as_deref())
    .bind(patch.selector != Change::Keep)
    .bind(match &patch.selector {
        Change::Set(selector) => Some(selector.as_str()),
        Change::Keep | Change::Clear => None,
    })
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
}
