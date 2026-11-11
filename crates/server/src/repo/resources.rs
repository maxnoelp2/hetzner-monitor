use hm_hcloud::{RawResource, ResourceKind};
use serde_json::Value;
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::OrgId;

#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct ResourceView {
    pub id: Uuid,
    pub kind: String,
    pub hetzner_id: i64,
    pub name: String,
    pub labels: Json<Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub deleted_at: Option<OffsetDateTime>,
}

/// A resource as the rule engine sees it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ResourceRow {
    pub id: Uuid,
    pub kind: String,
    pub hetzner_id: i64,
    pub name: String,
    pub labels: Json<Value>,
    pub snapshot: Json<Value>,
}

impl ResourceRow {
    /// Labels as a map (empty if the stored value is not an object).
    pub fn label_map(&self) -> serde_json::Map<String, Value> {
        self.labels.0.as_object().cloned().unwrap_or_default()
    }
}

/// Result of applying one kind: counts plus the ids of created and deleted resources.
#[derive(Debug, Default)]
pub struct Applied {
    pub counts: KindCounts,
    pub created: Vec<Uuid>,
    pub deleted: Vec<Uuid>,
}

/// Counts for one kind after applying a sync.
#[derive(Debug, Default, Clone, Copy, serde::Serialize, PartialEq, Eq)]
pub struct KindCounts {
    pub seen: u64,
    pub new: u64,
    pub deleted: u64,
}

/// Upserts all `items` of `kind` and marks resources that vanished as deleted.
/// Must run inside the caller's transaction; `project_id` must already be tenant-checked.
pub async fn apply(
    conn: &mut PgConnection,
    project_id: Uuid,
    kind: ResourceKind,
    items: &[RawResource],
    now: OffsetDateTime,
) -> Result<Applied, sqlx::Error> {
    let mut applied = Applied {
        counts: KindCounts {
            seen: items.len() as u64,
            ..KindCounts::default()
        },
        ..Applied::default()
    };
    for item in items {
        let (id, inserted): (Uuid, bool) = sqlx::query_as(
            "INSERT INTO resources (project_id, kind, hetzner_id, name, labels, snapshot, first_seen_at, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
             ON CONFLICT (project_id, kind, hetzner_id) DO UPDATE SET
                 name = EXCLUDED.name, labels = EXCLUDED.labels, snapshot = EXCLUDED.snapshot,
                 last_seen_at = EXCLUDED.last_seen_at, deleted_at = NULL
             RETURNING id, (xmax = 0)",
        )
        .bind(project_id)
        .bind(kind.as_str())
        .bind(item.id)
        .bind(&item.name)
        .bind(Json(Value::Object(item.labels.clone())))
        .bind(Json(&item.raw))
        .bind(now)
        .fetch_one(&mut *conn)
        .await?;
        if inserted {
            applied.counts.new += 1;
            applied.created.push(id);
        }
    }
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    applied.deleted = sqlx::query_scalar(
        "UPDATE resources SET deleted_at = $3
         WHERE project_id = $1 AND kind = $2 AND deleted_at IS NULL AND NOT (hetzner_id = ANY($4))
         RETURNING id",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(now)
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await?;
    applied.counts.deleted = applied.deleted.len() as u64;
    Ok(applied)
}

/// True if `resource_id` is a resource of `project_id` and the project belongs to `org`.
pub async fn belongs(
    db: &PgPool,
    org: OrgId,
    project_id: Uuid,
    resource_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM resources r JOIN hetzner_projects p ON p.id = r.project_id
                        WHERE r.id = $1 AND r.project_id = $2 AND p.org_id = $3)",
    )
    .bind(resource_id)
    .bind(project_id)
    .bind(org.0)
    .fetch_one(db)
    .await
}

/// Current (not deleted) resources of the given kinds. `project_id` must be tenant-checked.
pub async fn active(
    conn: &mut PgConnection,
    project_id: Uuid,
    kinds: &[&str],
) -> Result<Vec<ResourceRow>, sqlx::Error> {
    let kinds: Vec<String> = kinds.iter().map(|k| (*k).to_owned()).collect();
    sqlx::query_as(
        "SELECT id, kind, hetzner_id, name, labels, snapshot FROM resources
         WHERE project_id = $1 AND kind = ANY($2) AND deleted_at IS NULL
         ORDER BY kind, hetzner_id",
    )
    .bind(project_id)
    .bind(&kinds)
    .fetch_all(&mut *conn)
    .await
}

/// Resources of a project by id (deleted ones included). `project_id` must be tenant-checked.
pub async fn by_ids(
    conn: &mut PgConnection,
    project_id: Uuid,
    ids: &[Uuid],
) -> Result<Vec<ResourceRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT id, kind, hetzner_id, name, labels, snapshot FROM resources
         WHERE project_id = $1 AND id = ANY($2) ORDER BY kind, hetzner_id",
    )
    .bind(project_id)
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
}

/// Number of current resources per kind. `project_id` must be tenant-checked.
pub async fn counts_by_kind(
    conn: &mut PgConnection,
    project_id: Uuid,
) -> Result<Vec<(String, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT kind, count(*) FROM resources WHERE project_id = $1 AND deleted_at IS NULL GROUP BY kind",
    )
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await
}

/// Resources of a project of this org, optionally filtered by kind.
pub async fn list(
    db: &PgPool,
    org: OrgId,
    project_id: Uuid,
    kind: Option<ResourceKind>,
) -> Result<Vec<ResourceView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id, r.kind, r.hetzner_id, r.name, r.labels, r.first_seen_at, r.last_seen_at, r.deleted_at
         FROM resources r JOIN hetzner_projects p ON p.id = r.project_id
         WHERE p.org_id = $1 AND r.project_id = $2 AND ($3::text IS NULL OR r.kind = $3)
         ORDER BY r.kind, r.name, r.hetzner_id",
    )
    .bind(org.0)
    .bind(project_id)
    .bind(kind.map(ResourceKind::as_str))
    .fetch_all(db)
    .await
}
