use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use super::bad_path;
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::repo;
use crate::repo::alerts::{Acked, Filter};

const MAX_PER_PAGE: u32 = 100;
const DEFAULT_PER_PAGE: u32 = 25;

#[derive(Deserialize)]
pub struct ListQuery {
    /// `open`, `pending`, `firing` or `resolved`.
    state: Option<String>,
    project_id: Option<Uuid>,
    page: Option<u32>,
    per_page: Option<u32>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Query(query) = query.map_err(|r| ApiError::bad_request(r.body_text()))?;
    if let Some(s) = &query.state
        && !matches!(s.as_str(), "open" | "pending" | "firing" | "resolved")
    {
        return Err(ApiError::bad_request(
            "`state` must be open, pending, firing or resolved",
        ));
    }
    let page = query.page.unwrap_or(1).max(1);
    let per_page = query
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let filter = Filter {
        state: query.state,
        project_id: query.project_id,
    };
    let (alerts, total) =
        repo::alerts::list(&state.db, principal.org, &filter, page, per_page).await?;
    let total_u = u64::try_from(total).unwrap_or(0).max(1);
    let last_page = u32::try_from(total_u.div_ceil(u64::from(per_page))).unwrap_or(u32::MAX);
    Ok(Json(json!({
        "alerts": alerts,
        "meta": { "pagination": {
            "page": page,
            "per_page": per_page,
            "previous_page": (page > 1).then(|| page - 1),
            "next_page": (page < last_page).then(|| page + 1),
            "last_page": last_page,
            "total_entries": total,
        }},
    })))
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let alert = repo::alerts::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let events = repo::alerts::events(&state.db, principal.org, id).await?;
    let notifications = repo::notifications::for_alert(&state.db, principal.org, id).await?;
    Ok(Json(
        json!({ "alert": alert, "events": events, "notifications": notifications }),
    ))
}

/// `POST /alerts/{id}/ack`: stops re-notify; `resolved` is still sent. Idempotent.
pub async fn ack(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let mut tx = state.db.begin().await?;
    match repo::alerts::ack(
        &mut tx,
        principal.org,
        id,
        principal.user,
        OffsetDateTime::now_utc(),
    )
    .await?
    {
        Acked::Ok => {}
        Acked::NotFound => return Err(ApiError::not_found()),
        Acked::Resolved => {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "alert_resolved",
                "resolved alerts cannot be acknowledged",
            ));
        }
    }
    tx.commit().await?;
    let alert = repo::alerts::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({ "alert": alert })))
}
