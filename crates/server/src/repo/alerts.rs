//! Alerts and alert events. Write functions take ids obtained from a tenant-scoped read
//! ([`open_for_project`]) inside the same transaction; reads for the API take an [`OrgId`].

use serde_json::{Value, json};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{OrgId, UserId, notifications};

/// An alert that is not resolved yet, as the rule engine needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct OpenAlert {
    pub id: Uuid,
    pub rule_id: Uuid,
    pub resource_id: Option<Uuid>,
    pub state: String,
    pub pending_since: OffsetDateTime,
    pub ok_cycles: i32,
    pub fired_at: Option<OffsetDateTime>,
    pub last_notified_at: Option<OffsetDateTime>,
    pub acked_at: Option<OffsetDateTime>,
}

/// Open alerts of a project of this org.
pub async fn open_for_project(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
) -> Result<Vec<OpenAlert>, sqlx::Error> {
    sqlx::query_as(
        "SELECT a.id, a.rule_id, a.resource_id, a.state, a.pending_since, a.ok_cycles, a.fired_at,
                a.last_notified_at, a.acked_at
         FROM alerts a JOIN hetzner_projects p ON p.id = a.project_id
         WHERE p.org_id = $1 AND a.project_id = $2 AND a.state <> 'resolved'",
    )
    .bind(org.0)
    .bind(project_id)
    .fetch_all(&mut *conn)
    .await
}

/// A new alert. `state` is `pending`, `firing` or `resolved` (one-shot events).
pub struct NewAlert<'a> {
    pub rule_id: Uuid,
    pub resource_id: Option<Uuid>,
    pub project_id: Uuid,
    pub state: &'a str,
    pub severity: &'a str,
    pub value: Option<f64>,
    pub message: &'a str,
}

/// Inserts an alert and the events that lead to its state.
pub async fn insert(
    conn: &mut PgConnection,
    alert: &NewAlert<'_>,
    now: OffsetDateTime,
) -> Result<Uuid, sqlx::Error> {
    let fired = matches!(alert.state, "firing" | "resolved");
    let resolved = alert.state == "resolved";
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO alerts (rule_id, resource_id, project_id, state, severity, value, message,
                             pending_since, fired_at, resolved_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8,
                 CASE WHEN $9 THEN $8 END, CASE WHEN $10 THEN $8 END, $8)
         RETURNING id",
    )
    .bind(alert.rule_id)
    .bind(alert.resource_id)
    .bind(alert.project_id)
    .bind(alert.state)
    .bind(alert.severity)
    .bind(alert.value)
    .bind(alert.message)
    .bind(now)
    .bind(fired)
    .bind(resolved)
    .fetch_one(&mut *conn)
    .await?;
    let data = json!({ "message": alert.message, "value": alert.value });
    let first = if fired { "fired" } else { "pending" };
    let first_id = event(conn, id, first, now, &data).await?;
    if fired {
        notifications::enqueue(conn, first_id, id, "fired", now).await?;
    }
    if resolved {
        // One-shot alert (fired and resolved in one cycle): notified once, by `fired`.
        event(conn, id, "resolved", now, &data).await?;
    }
    Ok(id)
}

/// Refreshes value and message of an open alert without changing its state.
pub async fn touch(
    conn: &mut PgConnection,
    id: Uuid,
    value: Option<f64>,
    message: &str,
    ok_cycles: u32,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE alerts SET value = $2, message = $3, ok_cycles = $4, updated_at = $5
         WHERE id = $1 AND state <> 'resolved'",
    )
    .bind(id)
    .bind(value)
    .bind(message)
    .bind(i32::try_from(ok_cycles).unwrap_or(i32::MAX))
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// `pending → firing`.
pub async fn fire(
    conn: &mut PgConnection,
    id: Uuid,
    value: Option<f64>,
    message: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE alerts SET state = 'firing', fired_at = $4, value = $2, message = $3, ok_cycles = 0,
             updated_at = $4
         WHERE id = $1 AND state = 'pending'",
    )
    .bind(id)
    .bind(value)
    .bind(message)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let event_id = event(
        conn,
        id,
        "fired",
        now,
        &json!({ "message": message, "value": value }),
    )
    .await?;
    notifications::enqueue(conn, event_id, id, "fired", now).await?;
    Ok(())
}

/// `firing → resolved`.
pub async fn resolve(
    conn: &mut PgConnection,
    id: Uuid,
    value: Option<f64>,
    message: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE alerts SET state = 'resolved', resolved_at = $4, value = $2, message = $3, updated_at = $4
         WHERE id = $1 AND state = 'firing'",
    )
    .bind(id)
    .bind(value)
    .bind(message)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    let event_id = event(
        conn,
        id,
        "resolved",
        now,
        &json!({ "message": message, "value": value }),
    )
    .await?;
    notifications::enqueue(conn, event_id, id, "resolved", now).await?;
    Ok(())
}

/// A still-firing alert notifies again (re-notify, ADR-0012).
pub async fn renotify(
    conn: &mut PgConnection,
    id: Uuid,
    value: Option<f64>,
    message: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let event_id = event(
        conn,
        id,
        "renotified",
        now,
        &json!({ "message": message, "value": value }),
    )
    .await?;
    notifications::enqueue(conn, event_id, id, "renotified", now).await?;
    Ok(())
}

/// `pending → ok`: drops an alert that never fired (nothing was or will be notified).
pub async fn cancel(conn: &mut PgConnection, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM alerts WHERE id = $1 AND state = 'pending'")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

async fn event(
    conn: &mut PgConnection,
    alert_id: Uuid,
    kind: &str,
    at: OffsetDateTime,
    data: &Value,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO alert_events (alert_id, type, at, data) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(alert_id)
    .bind(kind)
    .bind(at)
    .bind(Json(data))
    .fetch_one(&mut *conn)
    .await
}

/// Outcome of [`ack`].
pub enum Acked {
    /// Acknowledged now or earlier (idempotent).
    Ok,
    NotFound,
    /// Resolved alerts cannot be acknowledged.
    Resolved,
}

/// Acknowledges an open alert of this org: stops re-notify; `resolved` is still sent.
pub async fn ack(
    conn: &mut PgConnection,
    org: OrgId,
    id: Uuid,
    user: UserId,
    now: OffsetDateTime,
) -> Result<Acked, sqlx::Error> {
    let row: Option<(String, Option<OffsetDateTime>)> = sqlx::query_as(
        "SELECT a.state, a.acked_at FROM alerts a JOIN hetzner_projects p ON p.id = a.project_id
         WHERE p.org_id = $1 AND a.id = $2 FOR UPDATE OF a",
    )
    .bind(org.0)
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    match row {
        None => Ok(Acked::NotFound),
        Some((state, _)) if state == "resolved" => Ok(Acked::Resolved),
        Some((_, Some(_))) => Ok(Acked::Ok),
        Some((_, None)) => {
            sqlx::query(
                "UPDATE alerts SET acked_at = $2, acked_by = $3, updated_at = $2 WHERE id = $1",
            )
            .bind(id)
            .bind(now)
            .bind(user.0)
            .execute(&mut *conn)
            .await?;
            event(conn, id, "acked", now, &json!({ "by": user.0 })).await?;
            Ok(Acked::Ok)
        }
    }
}

/// Closes the open alerts of a rule of this org (used when a rule is disabled):
/// firing ones resolve, pending ones are dropped.
pub async fn close_for_rule(
    conn: &mut PgConnection,
    org: OrgId,
    rule_id: Uuid,
    message: &str,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let open: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT a.id, a.state FROM alerts a JOIN rules r ON r.id = a.rule_id
         WHERE r.org_id = $1 AND a.rule_id = $2 AND a.state <> 'resolved'",
    )
    .bind(org.0)
    .bind(rule_id)
    .fetch_all(&mut *conn)
    .await?;
    for (id, state) in open {
        if state == "pending" {
            cancel(conn, id).await?;
        } else {
            resolve(conn, id, None, message, now).await?;
        }
    }
    Ok(())
}

/// An alert as shown in the API.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct AlertView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub rule_id: Uuid,
    pub rule_name: String,
    pub check_type: String,
    pub resource_id: Option<Uuid>,
    pub resource_kind: Option<String>,
    pub resource_name: Option<String>,
    pub hetzner_id: Option<i64>,
    pub state: String,
    pub severity: String,
    pub value: Option<f64>,
    pub message: String,
    #[serde(with = "time::serde::rfc3339")]
    pub pending_since: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub fired_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub resolved_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub acked_at: Option<OffsetDateTime>,
    pub acked_by: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_notified_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

macro_rules! view_select {
    () => {
        "SELECT a.id, a.project_id, a.rule_id, r.name AS rule_name, r.check_type, a.resource_id,
                res.kind AS resource_kind, res.name AS resource_name, res.hetzner_id, a.state, a.severity,
                a.value, a.message, a.pending_since, a.fired_at, a.resolved_at, a.acked_at, a.acked_by,
                a.last_notified_at, a.updated_at
         FROM alerts a
         JOIN hetzner_projects p ON p.id = a.project_id
         JOIN rules r ON r.id = a.rule_id
         LEFT JOIN resources res ON res.id = a.resource_id"
    };
}

/// Filter for [`list`]. `state`: `open` (pending or firing), `pending`, `firing` or `resolved`.
#[derive(Debug, Default)]
pub struct Filter {
    pub state: Option<String>,
    pub project_id: Option<Uuid>,
}

/// One page of the org's alerts, newest first, plus the total count.
pub async fn list(
    db: &PgPool,
    org: OrgId,
    filter: &Filter,
    page: u32,
    per_page: u32,
) -> Result<(Vec<AlertView>, i64), sqlx::Error> {
    let offset = i64::from(page.saturating_sub(1)) * i64::from(per_page);
    let alerts = sqlx::query_as(concat!(
        view_select!(),
        " WHERE p.org_id = $1
            AND ($2::text IS NULL OR ($2 = 'open' AND a.state <> 'resolved') OR a.state = $2)
            AND ($3::uuid IS NULL OR a.project_id = $3)
          ORDER BY a.updated_at DESC, a.id
          LIMIT $4 OFFSET $5"
    ))
    .bind(org.0)
    .bind(filter.state.as_deref())
    .bind(filter.project_id)
    .bind(i64::from(per_page))
    .bind(offset)
    .fetch_all(db)
    .await?;
    let total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM alerts a JOIN hetzner_projects p ON p.id = a.project_id
         WHERE p.org_id = $1
           AND ($2::text IS NULL OR ($2 = 'open' AND a.state <> 'resolved') OR a.state = $2)
           AND ($3::uuid IS NULL OR a.project_id = $3)",
    )
    .bind(org.0)
    .bind(filter.state.as_deref())
    .bind(filter.project_id)
    .fetch_one(db)
    .await?;
    Ok((alerts, total))
}

pub async fn get(db: &PgPool, org: OrgId, id: Uuid) -> Result<Option<AlertView>, sqlx::Error> {
    sqlx::query_as(concat!(
        view_select!(),
        " WHERE p.org_id = $1 AND a.id = $2"
    ))
    .bind(org.0)
    .bind(id)
    .fetch_optional(db)
    .await
}

/// An alert event as shown in the API.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct EventView {
    pub id: i64,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub data: Json<Value>,
}

/// Events of an alert of this org, oldest first.
pub async fn events(
    db: &PgPool,
    org: OrgId,
    alert_id: Uuid,
) -> Result<Vec<EventView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT e.id, e.type, e.at, e.data FROM alert_events e
         JOIN alerts a ON a.id = e.alert_id JOIN hetzner_projects p ON p.id = a.project_id
         WHERE p.org_id = $1 AND e.alert_id = $2 ORDER BY e.id",
    )
    .bind(org.0)
    .bind(alert_id)
    .fetch_all(db)
    .await
}
