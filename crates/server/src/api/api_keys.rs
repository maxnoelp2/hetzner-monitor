use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hm_core::apikey;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::repo;

#[derive(Deserialize)]
pub struct CreateKey {
    name: String,
    scopes: Vec<String>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Admin)?;
    let keys = repo::api_keys::list(&state.db, principal.org).await?;
    Ok(Json(json!({ "api_keys": keys })))
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateKey>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    principal.require(Scope::Admin)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request("`name` must be 1–100 characters"));
    }
    let mut scopes = Vec::new();
    for raw in &body.scopes {
        let scope = Scope::parse(raw)
            .ok_or_else(|| ApiError::bad_request(format!("unknown scope `{raw}`")))?;
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    if scopes.is_empty() {
        return Err(ApiError::bad_request(
            "`scopes` must contain read, write and/or admin",
        ));
    }
    let key = apikey::generate().map_err(|e| ApiError::internal(&e))?;
    let view = repo::api_keys::insert(
        &state.db,
        principal.org,
        principal.user,
        name,
        &scopes,
        &key,
    )
    .await?;
    // The only time the full key leaves the server.
    Ok((
        StatusCode::CREATED,
        Json(json!({ "api_key": view, "key": key.full.expose().as_str() })),
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Admin)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::api_keys::revoke(&state.db, principal.org, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
