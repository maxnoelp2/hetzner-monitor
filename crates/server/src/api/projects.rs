use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use hm_hcloud::{ResourceKind, Token};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::{inventory, repo};

#[derive(Deserialize)]
pub struct CreateProject {
    name: String,
    token: String,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let projects = repo::projects::list(&state.db, principal.org).await?;
    Ok(Json(json!({ "projects": projects })))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateProject>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    principal.require(Scope::Write)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::bad_request("`name` must be 1–200 characters"));
    }
    let token = body.token.trim();
    if token.is_empty() || token.len() > 512 {
        return Err(ApiError::bad_request(
            "`token` must be a Hetzner Cloud API token",
        ));
    }
    let (project, sync) =
        inventory::connect(&state, principal.org, name, Token::new(token.to_owned())).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "project": project, "sync": sync })),
    ))
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let project = repo::projects::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let cost = repo::snapshots::latest_cost(&state.db, principal.org, id).await?;
    let mut project = serde_json::to_value(project).map_err(|e| ApiError::internal(&e))?;
    project["cost_estimate"] = serde_json::to_value(cost).map_err(|e| ApiError::internal(&e))?;
    Ok(Json(json!({ "project": project })))
}

pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::projects::delete(&state.db, principal.org, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

pub async fn sync(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let report = inventory::sync(&state, principal.org, id).await?;
    let project = repo::projects::get(&state.db, principal.org, id).await?;
    Ok(Json(json!({ "sync": report, "project": project })))
}

#[derive(Deserialize)]
pub struct ResourceQuery {
    kind: Option<String>,
}

pub async fn resources(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::projects::get(&state.db, principal.org, id)
        .await?
        .is_none()
    {
        return Err(ApiError::not_found());
    }
    let kind = query
        .kind
        .map(|k| k.parse::<ResourceKind>())
        .transpose()
        .map_err(ApiError::bad_request)?;
    let resources = repo::resources::list(&state.db, principal.org, id, kind).await?;
    Ok(Json(json!({ "resources": resources })))
}
