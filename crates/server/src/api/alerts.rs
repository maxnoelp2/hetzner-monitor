use axum::Json;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::bad_path;
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::openapi::ErrorBody;
use crate::repo;
use crate::repo::alerts::{Acked, AlertView, EventView, Filter};
use crate::repo::notifications::JobView;

const MAX_PER_PAGE: u32 = 100;
const DEFAULT_PER_PAGE: u32 = 25;

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// `open` = `pending` or `firing`.
    #[param(value_type = Option<crate::openapi::enums::AlertFilter>)]
    state: Option<String>,
    project_id: Option<Uuid>,
    /// Default 1.
    #[param(minimum = 1)]
    page: Option<u32>,
    /// Default 25, at most 100.
    #[param(minimum = 1, maximum = 100)]
    per_page: Option<u32>,
}

/// Pagination in Hetzner's shape.
#[derive(Serialize, ToSchema)]
pub struct Pagination {
    page: u32,
    per_page: u32,
    previous_page: Option<u32>,
    next_page: Option<u32>,
    last_page: u32,
    total_entries: i64,
}

#[derive(Serialize, ToSchema)]
pub struct Meta {
    pagination: Pagination,
}

#[derive(Serialize, ToSchema)]
pub struct AlertList {
    alerts: Vec<AlertView>,
    meta: Meta,
}

#[derive(Serialize, ToSchema)]
pub struct AlertEnvelope {
    alert: AlertView,
}

/// An alert with its history and delivery jobs.
#[derive(Serialize, ToSchema)]
pub struct AlertDetail {
    alert: AlertView,
    events: Vec<EventView>,
    notifications: Vec<JobView>,
}

/// List alerts, newest first, paginated. Scope: `read`.
#[utoipa::path(get, path = "/alerts", operation_id = "listAlerts", tag = "alerts",
    params(ListQuery),
    responses(
        (status = OK, body = AlertList),
        (status = BAD_REQUEST, description = "Invalid query (`bad_request`)", body = ErrorBody),
    ))]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<AlertList>, ApiError> {
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
    Ok(Json(AlertList {
        alerts,
        meta: Meta {
            pagination: Pagination {
                page,
                per_page,
                previous_page: (page > 1).then(|| page - 1),
                next_page: (page < last_page).then(|| page + 1),
                last_page,
                total_entries: total,
            },
        },
    }))
}

/// An alert with its events and notification deliveries. Scope: `read`.
#[utoipa::path(get, path = "/alerts/{id}", operation_id = "getAlert", tag = "alerts",
    params(("id" = Uuid, Path, description = "Alert id")),
    responses(
        (status = OK, body = AlertDetail),
        (status = NOT_FOUND, description = "Unknown alert (`not_found`)", body = ErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<AlertDetail>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let alert = repo::alerts::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let events = repo::alerts::events(&state.db, principal.org, id).await?;
    let notifications = repo::notifications::for_alert(&state.db, principal.org, id).await?;
    Ok(Json(AlertDetail {
        alert,
        events,
        notifications,
    }))
}

/// Acknowledge an alert: stops re-notify; `resolved` is still sent. Idempotent. Scope: `write`.
#[utoipa::path(post, path = "/alerts/{id}/ack", operation_id = "ackAlert", tag = "alerts",
    params(("id" = Uuid, Path, description = "Alert id")),
    responses(
        (status = OK, body = AlertEnvelope),
        (status = NOT_FOUND, description = "Unknown alert (`not_found`)", body = ErrorBody),
        (status = CONFLICT, description = "Already resolved (`alert_resolved`)", body = ErrorBody),
    ))]
pub async fn ack(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<AlertEnvelope>, ApiError> {
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
    Ok(Json(AlertEnvelope { alert }))
}
