//! `/silences`: suppress notifications for a scope and time window.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::rules::valid_selector;
use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::openapi::ErrorBody;
use crate::repo;
use crate::repo::silences::{Inserted, NewSilence, SilenceView};

#[derive(Serialize, ToSchema)]
pub struct SilenceList {
    silences: Vec<SilenceView>,
}

#[derive(Serialize, ToSchema)]
pub struct SilenceEnvelope {
    silence: SilenceView,
}

/// Longest silence.
const MAX_SILENCE: Duration = Duration::days(365);

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    /// Default `true`: only silences that have not ended; `false` = all.
    active: Option<bool>,
}

/// List silences. Scope: `read`.
#[utoipa::path(get, path = "/silences", operation_id = "listSilences", tag = "silences",
    params(ListQuery),
    responses(
        (status = OK, body = SilenceList),
        (status = BAD_REQUEST, description = "Invalid query (`bad_request`)", body = ErrorBody),
    ))]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    query: Result<Query<ListQuery>, QueryRejection>,
) -> Result<Json<SilenceList>, ApiError> {
    principal.require(Scope::Read)?;
    let Query(query) = query.map_err(|r| ApiError::bad_request(r.body_text()))?;
    let silences = repo::silences::list(
        &state.db,
        principal.org,
        query.active.unwrap_or(true),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(SilenceList { silences }))
}

/// All scope fields are optional; none = the whole organisation.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSilence {
    project_id: Option<Uuid>,
    resource_id: Option<Uuid>,
    rule_id: Option<Uuid>,
    /// Label selector matched against resource labels.
    selector: Option<String>,
    /// Default: now.
    #[serde(default, with = "time::serde::rfc3339::option")]
    starts_at: Option<OffsetDateTime>,
    /// In the future, at most 365 days after `starts_at`.
    #[serde(with = "time::serde::rfc3339")]
    ends_at: OffsetDateTime,
    /// At most 500 characters.
    #[serde(default)]
    reason: String,
}

/// Create a silence. Scope: `write`.
#[utoipa::path(post, path = "/silences", operation_id = "createSilence", tag = "silences",
    request_body = CreateSilence,
    responses(
        (status = CREATED, body = SilenceEnvelope),
        (status = BAD_REQUEST, description = "Invalid body, window, selector or scope (`bad_request`)", body = ErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateSilence>, JsonRejection>,
) -> Result<(StatusCode, Json<SilenceEnvelope>), ApiError> {
    principal.require(Scope::Write)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let now = OffsetDateTime::now_utc();
    let starts_at = body.starts_at.unwrap_or(now);
    if body.ends_at <= starts_at.max(now) {
        return Err(ApiError::bad_request(
            "`ends_at` must be after `starts_at` and in the future",
        ));
    }
    if body.ends_at - starts_at > MAX_SILENCE {
        return Err(ApiError::bad_request("a silence lasts at most 365 days"));
    }
    if body.reason.chars().count() > 500 {
        return Err(ApiError::bad_request("`reason` must be ≤ 500 characters"));
    }
    let selector = body
        .selector
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(valid_selector)
        .transpose()?;
    let mut tx = state.db.begin().await?;
    let inserted = repo::silences::insert(
        &mut tx,
        principal.org,
        principal.user,
        &NewSilence {
            project_id: body.project_id,
            resource_id: body.resource_id,
            rule_id: body.rule_id,
            selector: selector.as_deref(),
            starts_at,
            ends_at: body.ends_at,
            reason: body.reason.trim(),
        },
        now,
    )
    .await?;
    let Inserted::Ok(silence) = inserted else {
        return Err(ApiError::bad_request(
            "`project_id`, `resource_id` or `rule_id` is not part of this organisation",
        ));
    };
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(SilenceEnvelope { silence: *silence }),
    ))
}

/// End a silence now (a future one is removed). Scope: `write`.
#[utoipa::path(delete, path = "/silences/{id}", operation_id = "deleteSilence", tag = "silences",
    params(("id" = Uuid, Path, description = "Silence id")),
    responses(
        (status = NO_CONTENT, description = "Ended"),
        (status = NOT_FOUND, description = "Unknown silence (`not_found`)", body = ErrorBody),
    ))]
pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::silences::end(&state.db, principal.org, id, OffsetDateTime::now_utc()).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
