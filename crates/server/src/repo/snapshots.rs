//! Derived daily snapshots (ADR-0011): the project's estimated monthly cost per day and the
//! first outgoing-traffic counter per server / load balancer and day. Both are our own derived
//! values with at most one row per day; no metric series is stored (invariant 8).

use std::collections::HashMap;

use hm_core::checks::{CostPoint, TrafficSample};
use hm_core::cost::Estimate;
use sqlx::{PgConnection, PgPool};
use time::{Date, Duration, OffsetDateTime};
use uuid::Uuid;

use super::OrgId;

/// Days a cost estimate is kept.
pub const COST_RETENTION_DAYS: i64 = 90;
/// Days a traffic sample is kept (a billing month plus margin).
pub const TRAFFIC_RETENTION_DAYS: i64 = 40;

/// The latest cost estimate of a project, as shown in the API.
#[derive(Debug, sqlx::FromRow, serde::Serialize, utoipa::ToSchema)]
pub struct CostView {
    /// UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// Net, in `currency`; see docs/design/checks.md#cost-estimate for what it includes.
    pub monthly_net: f64,
    pub currency: String,
    /// Resources without a price in `/pricing` (not included).
    pub unpriced: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// Stores today's cost estimate (replacing an earlier one of the same day) and drops old days.
pub async fn upsert_cost(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
    now: OffsetDateTime,
    estimate: &Estimate,
) -> Result<(), sqlx::Error> {
    let unpriced = i32::try_from(estimate.unpriced.len()).unwrap_or(i32::MAX);
    sqlx::query(
        "INSERT INTO cost_estimates (project_id, day, monthly_net, currency, unpriced, at)
         SELECT p.id, $3, $4, $5, $6, $7 FROM hetzner_projects p WHERE p.id = $1 AND p.org_id = $2
         ON CONFLICT (project_id, day) DO UPDATE SET monthly_net = EXCLUDED.monthly_net,
             currency = EXCLUDED.currency, unpriced = EXCLUDED.unpriced, at = EXCLUDED.at",
    )
    .bind(project_id)
    .bind(org.0)
    .bind(now.date())
    .bind(estimate.monthly_net)
    .bind(&estimate.currency)
    .bind(unpriced)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM cost_estimates c USING hetzner_projects p
         WHERE c.project_id = p.id AND p.id = $1 AND p.org_id = $2 AND c.day < $3",
    )
    .bind(project_id)
    .bind(org.0)
    .bind((now - Duration::days(COST_RETENTION_DAYS)).date())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Today's estimate and the latest estimate of an earlier day.
pub async fn cost_points(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
    today: Date,
) -> Result<(Option<CostPoint>, Option<CostPoint>), sqlx::Error> {
    let rows: Vec<(Date, f64, String)> = sqlx::query_as(
        "(SELECT c.day, c.monthly_net, c.currency FROM cost_estimates c
          JOIN hetzner_projects p ON p.id = c.project_id
          WHERE p.id = $1 AND p.org_id = $2 AND c.day = $3)
         UNION ALL
         (SELECT c.day, c.monthly_net, c.currency FROM cost_estimates c
          JOIN hetzner_projects p ON p.id = c.project_id
          WHERE p.id = $1 AND p.org_id = $2 AND c.day < $3 ORDER BY c.day DESC LIMIT 1)",
    )
    .bind(project_id)
    .bind(org.0)
    .bind(today)
    .fetch_all(&mut *conn)
    .await?;
    let point = |(day, monthly_net, currency): (Date, f64, String)| CostPoint {
        day: day.to_string(),
        monthly_net,
        currency,
    };
    let (mut current, mut previous) = (None, None);
    for row in rows {
        if row.0 == today {
            current = Some(point(row));
        } else {
            previous = Some(point(row));
        }
    }
    Ok((current, previous))
}

/// The latest cost estimate of a project of this org.
pub async fn latest_cost(
    db: &PgPool,
    org: OrgId,
    project_id: Uuid,
) -> Result<Option<CostView>, sqlx::Error> {
    sqlx::query_as(
        "SELECT c.day::text AS day, c.monthly_net, c.currency, c.unpriced, c.at FROM cost_estimates c
         JOIN hetzner_projects p ON p.id = c.project_id
         WHERE p.id = $1 AND p.org_id = $2 ORDER BY c.day DESC LIMIT 1",
    )
    .bind(project_id)
    .bind(org.0)
    .fetch_optional(db)
    .await
}

/// Records `(resource id, outgoing bytes)` as the day's sample unless the day already has one,
/// and drops old samples. Resources outside the project or org are ignored.
pub async fn record_traffic(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
    now: OffsetDateTime,
    samples: &[(Uuid, i64)],
) -> Result<(), sqlx::Error> {
    let (ids, bytes): (Vec<Uuid>, Vec<i64>) = samples.iter().copied().unzip();
    sqlx::query(
        "INSERT INTO traffic_samples (resource_id, day, at, outgoing_traffic)
         SELECT r.id, $3, $4, s.bytes
         FROM unnest($5::uuid[], $6::bigint[]) AS s (id, bytes)
         JOIN resources r ON r.id = s.id
         JOIN hetzner_projects p ON p.id = r.project_id
         WHERE r.project_id = $1 AND p.org_id = $2
         ON CONFLICT (resource_id, day) DO NOTHING",
    )
    .bind(project_id)
    .bind(org.0)
    .bind(now.date())
    .bind(now)
    .bind(&ids)
    .bind(&bytes)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM traffic_samples t USING resources r, hetzner_projects p
         WHERE t.resource_id = r.id AND r.project_id = p.id AND p.id = $1 AND p.org_id = $2
           AND t.day < $3",
    )
    .bind(project_id)
    .bind(org.0)
    .bind((now - Duration::days(TRAFFIC_RETENTION_DAYS)).date())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Per resource of the project: the newest traffic sample taken at or before `before`.
pub async fn traffic_baselines(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
    before: OffsetDateTime,
) -> Result<HashMap<Uuid, TrafficSample>, sqlx::Error> {
    let rows: Vec<(Uuid, OffsetDateTime, i64)> = sqlx::query_as(
        "SELECT DISTINCT ON (t.resource_id) t.resource_id, t.at, t.outgoing_traffic
         FROM traffic_samples t
         JOIN resources r ON r.id = t.resource_id
         JOIN hetzner_projects p ON p.id = r.project_id
         WHERE r.project_id = $1 AND p.org_id = $2 AND t.at <= $3
         ORDER BY t.resource_id, t.at DESC",
    )
    .bind(project_id)
    .bind(org.0)
    .bind(before)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, at, bytes)| {
            (
                id,
                TrafficSample {
                    at: at.unix_timestamp(),
                    outgoing: u64::try_from(bytes).unwrap_or(0),
                },
            )
        })
        .collect())
}
