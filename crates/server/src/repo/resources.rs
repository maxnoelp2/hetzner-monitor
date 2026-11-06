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
) -> Result<KindCounts, sqlx::Error> {
    let mut counts = KindCounts {
        seen: items.len() as u64,
        ..KindCounts::default()
    };
    for item in items {
        let inserted: bool = sqlx::query_scalar(
            "INSERT INTO resources (project_id, kind, hetzner_id, name, labels, snapshot, first_seen_at, last_seen_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
             ON CONFLICT (project_id, kind, hetzner_id) DO UPDATE SET
                 name = EXCLUDED.name, labels = EXCLUDED.labels, snapshot = EXCLUDED.snapshot,
                 last_seen_at = EXCLUDED.last_seen_at, deleted_at = NULL
             RETURNING (xmax = 0)",
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
            counts.new += 1;
        }
    }
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    counts.deleted = sqlx::query(
        "UPDATE resources SET deleted_at = $3
         WHERE project_id = $1 AND kind = $2 AND deleted_at IS NULL AND NOT (hetzner_id = ANY($4))",
    )
    .bind(project_id)
    .bind(kind.as_str())
    .bind(now)
    .bind(&ids)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(counts)
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
