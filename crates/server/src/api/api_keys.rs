use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hm_core::apikey;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::openapi::ErrorBody;
use crate::repo;
use crate::repo::api_keys::ApiKeyView;

#[derive(Serialize, ToSchema)]
pub struct ApiKeyList {
    api_keys: Vec<ApiKeyView>,
}

#[derive(Serialize, ToSchema)]
pub struct ApiKeyCreated {
    api_key: ApiKeyView,
    /// The full key (`hm_…`). Shown only in this response.
    key: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateKey {
    /// 1–100 characters.
    name: String,
    #[schema(value_type = Vec<crate::openapi::enums::Scope>, min_items = 1)]
    scopes: Vec<String>,
}

/// List API keys (including revoked ones). Scope: `admin`.
#[utoipa::path(get, path = "/api-keys", operation_id = "listApiKeys", tag = "api-keys",
    responses((status = OK, body = ApiKeyList)))]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<ApiKeyList>, ApiError> {
    principal.require(Scope::Admin)?;
    let keys = repo::api_keys::list(&state.db, principal.org).await?;
    Ok(Json(ApiKeyList { api_keys: keys }))
}

/// Create an API key; the key is returned once. Scope: `admin`.
#[utoipa::path(post, path = "/api-keys", operation_id = "createApiKey", tag = "api-keys",
    request_body = CreateKey,
    responses(
        (status = CREATED, body = ApiKeyCreated),
        (status = BAD_REQUEST, description = "Invalid name or scopes (`bad_request`)", body = ErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateKey>, JsonRejection>,
) -> Result<(StatusCode, Json<ApiKeyCreated>), ApiError> {
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
        Json(ApiKeyCreated {
            api_key: view,
            key: key.full.expose().as_str().to_owned(),
        }),
    ))
}

/// Revoke an API key. Scope: `admin`.
#[utoipa::path(delete, path = "/api-keys/{id}", operation_id = "revokeApiKey", tag = "api-keys",
    params(("id" = Uuid, Path, description = "API key id")),
    responses(
        (status = NO_CONTENT, description = "Revoked"),
        (status = NOT_FOUND, description = "Unknown key (`not_found`)", body = ErrorBody),
    ))]
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
