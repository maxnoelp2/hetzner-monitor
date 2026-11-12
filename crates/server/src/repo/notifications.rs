//! The notification queue (ADR-0012). [`enqueue`] runs inside the transaction that writes an
//! alert event; the notifier claims due jobs with [`claim_next`] and settles them.

use hm_core::selector::Selector;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use super::{OrgId, silences};

/// Days a finished (sent or failed) job is kept.
pub const JOB_RETENTION_DAYS: i64 = 90;

/// What [`enqueue`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enqueued {
    /// Jobs for these channels.
    Jobs(Vec<Uuid>),
    /// A silence covers the alert; a `suppressed` event was recorded.
    Suppressed(Uuid),
}

/// The alert facts routing and silences need.
#[derive(sqlx::FromRow)]
struct AlertFacts {
    org_id: Uuid,
    rule_id: Uuid,
    project_id: Uuid,
    resource_id: Option<Uuid>,
    severity: String,
    labels: Option<Json<Value>>,
}

/// Routes the alert event `event_id` (`fired`, `resolved` or `renotified`) of `alert_id`:
/// records a `suppressed` event if an active silence covers it, else inserts one queued job per
/// routed channel. `fired`/`renotified` also set `last_notified_at` (also when suppressed, so a
/// silence that ends does not cause an immediate re-notify).
pub async fn enqueue(
    conn: &mut PgConnection,
    event_id: i64,
    alert_id: Uuid,
    kind: &str,
    now: OffsetDateTime,
) -> Result<Enqueued, sqlx::Error> {
    let facts: AlertFacts = sqlx::query_as(
        "SELECT p.org_id, a.rule_id, a.project_id, a.resource_id, a.severity, r.labels
         FROM alerts a
         JOIN hetzner_projects p ON p.id = a.project_id
         LEFT JOIN resources r ON r.id = a.resource_id
         WHERE a.id = $1",
    )
    .bind(alert_id)
    .fetch_one(&mut *conn)
    .await?;
    let org = OrgId(facts.org_id);
    if kind != "resolved" {
        sqlx::query("UPDATE alerts SET last_notified_at = $2 WHERE id = $1")
            .bind(alert_id)
            .bind(now)
            .execute(&mut *conn)
            .await?;
    }

    let labels: Map<String, Value> = facts
        .labels
        .and_then(|l| l.0.as_object().cloned())
        .unwrap_or_default();
    let candidates = silences::covering(
        &mut *conn,
        org,
        facts.project_id,
        facts.resource_id,
        facts.rule_id,
        now,
    )
    .await?;
    let silence = candidates.into_iter().find_map(|(id, selector)| {
        let covers = selector.as_deref().is_none_or(|raw| {
            raw.parse::<Selector>()
                .is_ok_and(|sel| sel.matches(&labels))
        });
        covers.then_some(id)
    });
    if let Some(silence_id) = silence {
        sqlx::query(
            "INSERT INTO alert_events (alert_id, type, at, data) VALUES ($1, 'suppressed', $2, $3)",
        )
        .bind(alert_id)
        .bind(now)
        .bind(Json(json!({ "silence_id": silence_id, "event": kind })))
        .execute(&mut *conn)
        .await?;
        return Ok(Enqueued::Suppressed(silence_id));
    }

    let channels: Vec<Uuid> = if kind == "resolved" {
        // Only where the alert's fired / renotified notifications went.
        sqlx::query_scalar(
            "SELECT DISTINCT j.channel_id FROM notification_jobs j
             JOIN alert_events e ON e.id = j.alert_event_id
             JOIN notification_channels c ON c.id = j.channel_id
             WHERE e.alert_id = $1 AND e.type IN ('fired', 'renotified') AND c.enabled AND c.org_id = $2",
        )
        .bind(alert_id)
        .bind(org.0)
        .fetch_all(&mut *conn)
        .await?
    } else {
        // Pinned channels if the rule has any, else every enabled channel of the org whose
        // `min_severity` the alert reaches.
        sqlx::query_scalar(
            "WITH pinned AS (
                 SELECT c.id FROM rule_channels rc
                 JOIN notification_channels c ON c.id = rc.channel_id
                 WHERE rc.rule_id = $2 AND c.org_id = $1
             )
             SELECT c.id FROM notification_channels c
             WHERE c.org_id = $1 AND c.enabled
               AND CASE WHEN EXISTS (SELECT 1 FROM pinned) THEN c.id IN (SELECT id FROM pinned)
                        ELSE array_position(ARRAY['info','warning','critical'], c.min_severity)
                             <= array_position(ARRAY['info','warning','critical'], $3)
                   END
             ORDER BY c.created_at, c.id",
        )
        .bind(org.0)
        .bind(facts.rule_id)
        .bind(&facts.severity)
        .fetch_all(&mut *conn)
        .await?
    };
    if !channels.is_empty() {
        sqlx::query(
            "INSERT INTO notification_jobs (alert_event_id, channel_id, next_attempt_at, created_at)
             SELECT $1, c, $3, $3 FROM unnest($2::uuid[]) AS c
             ON CONFLICT (alert_event_id, channel_id) DO NOTHING",
        )
        .bind(event_id)
        .bind(&channels)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    Ok(Enqueued::Jobs(channels))
}

/// A claimed job with everything needed to build and send its payload. Its row stays locked
/// until the transaction that claimed it ends.
#[derive(Debug, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub attempts: i32,
    pub channel_id: Uuid,
    pub channel_type: String,
    pub channel_enabled: bool,
    pub config_ct: Vec<u8>,
    pub config_dek_ct: Vec<u8>,
    pub key_version: i32,
    pub event_type: String,
    pub event_at: OffsetDateTime,
    pub event_data: Json<Value>,
    pub alert_id: Uuid,
    pub severity: String,
    pub fired_at: Option<OffsetDateTime>,
    pub resolved_at: Option<OffsetDateTime>,
    pub rule_id: Uuid,
    pub rule_name: String,
    pub check_type: String,
    pub project_id: Uuid,
    pub project_name: String,
    pub resource_kind: Option<String>,
    pub hetzner_id: Option<i64>,
    pub resource_name: Option<String>,
}

/// Claims the oldest due job (any tenant: the notifier is a system task like the scheduler).
/// Skips jobs another worker holds. The channel must belong to the alert's org.
pub async fn claim_next(
    conn: &mut PgConnection,
    now: OffsetDateTime,
) -> Result<Option<Job>, sqlx::Error> {
    sqlx::query_as(
        "SELECT j.id, j.attempts, j.channel_id, c.type AS channel_type, c.enabled AS channel_enabled,
                c.config_ct, c.config_dek_ct, c.key_version,
                e.type AS event_type, e.at AS event_at, e.data AS event_data,
                a.id AS alert_id, a.severity, a.fired_at, a.resolved_at,
                r.id AS rule_id, r.name AS rule_name, r.check_type,
                p.id AS project_id, p.name AS project_name,
                res.kind AS resource_kind, res.hetzner_id, res.name AS resource_name
         FROM notification_jobs j
         JOIN notification_channels c ON c.id = j.channel_id
         JOIN alert_events e ON e.id = j.alert_event_id
         JOIN alerts a ON a.id = e.alert_id
         JOIN rules r ON r.id = a.rule_id
         JOIN hetzner_projects p ON p.id = a.project_id AND p.org_id = c.org_id
         LEFT JOIN resources res ON res.id = a.resource_id
         WHERE j.status = 'queued' AND j.next_attempt_at <= $1
         ORDER BY j.next_attempt_at, j.id
         LIMIT 1
         FOR UPDATE OF j SKIP LOCKED",
    )
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
}

/// Marks a job sent.
pub async fn mark_sent(
    conn: &mut PgConnection,
    id: i64,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_jobs SET status = 'sent', attempts = attempts + 1, finished_at = $2,
             last_error = NULL WHERE id = $1",
    )
    .bind(id)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Records a failed attempt: re-queued at `retry_at`, or `failed` when `retry_at` is `None`.
pub async fn mark_failed(
    conn: &mut PgConnection,
    id: i64,
    error: &str,
    retry_at: Option<OffsetDateTime>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE notification_jobs SET attempts = attempts + 1, last_error = $2,
             status = CASE WHEN $3::timestamptz IS NULL THEN 'failed' ELSE 'queued' END,
             next_attempt_at = coalesce($3, next_attempt_at),
             finished_at = CASE WHEN $3::timestamptz IS NULL THEN $4 END
         WHERE id = $1",
    )
    .bind(id)
    .bind(error)
    .bind(retry_at)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Deletes jobs finished more than [`JOB_RETENTION_DAYS`] ago. Returns how many.
pub async fn cleanup(db: &sqlx::PgPool, now: OffsetDateTime) -> Result<u64, sqlx::Error> {
    let done =
        sqlx::query("DELETE FROM notification_jobs WHERE status <> 'queued' AND finished_at < $1")
            .bind(now - time::Duration::days(JOB_RETENTION_DAYS))
            .execute(db)
            .await?;
    Ok(done.rows_affected())
}

/// A job as shown on `GET /alerts/{id}`.
#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct JobView {
    pub id: i64,
    pub alert_event_id: i64,
    pub channel_id: Uuid,
    pub channel_name: String,
    #[schema(value_type = crate::openapi::enums::JobStatus)]
    pub status: String,
    pub attempts: i32,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub next_attempt_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

/// Notification jobs of an alert of this org.
pub async fn for_alert(
    db: &sqlx::PgPool,
    org: OrgId,
    alert_id: Uuid,
) -> Result<Vec<JobView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT j.id, j.alert_event_id, j.channel_id, c.name AS channel_name, j.status, j.attempts,
                j.last_error, j.next_attempt_at, j.finished_at
         FROM notification_jobs j
         JOIN notification_channels c ON c.id = j.channel_id
         JOIN alert_events e ON e.id = j.alert_event_id
         JOIN alerts a ON a.id = e.alert_id
         JOIN hetzner_projects p ON p.id = a.project_id
         WHERE p.org_id = $1 AND c.org_id = $1 AND a.id = $2
         ORDER BY j.id",
    )
    .bind(org.0)
    .bind(alert_id)
    .fetch_all(db)
    .await
}
