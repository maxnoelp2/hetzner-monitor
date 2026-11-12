use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use hm_hcloud::{ResourceKind, Token};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::cycle::SyncReport;
use crate::error::ApiError;
use crate::http::AppState;
use crate::openapi::ErrorBody;
use crate::repo::projects::ProjectView;
use crate::repo::resources::{KindCounts, ResourceView};
use crate::repo::snapshots::CostView;
use crate::{inventory, repo};

#[derive(Deserialize, ToSchema)]
pub struct CreateProject {
    /// 1–200 characters, unique per organisation.
    name: String,
    /// A **read-only** Hetzner Cloud API token of the project. Write-only: never returned.
    token: String,
}

#[derive(Serialize, ToSchema)]
pub struct ProjectList {
    projects: Vec<ProjectView>,
}

/// Resources seen in an inventory cycle, per kind (`server`, `load_balancer`, …).
#[derive(Serialize, ToSchema)]
#[schema(as = SyncReport)]
pub struct SyncReportDoc(#[schema(inline)] std::collections::BTreeMap<String, KindCounts>);

#[derive(Serialize, ToSchema)]
pub struct ProjectCreated {
    project: ProjectView,
    /// The first inventory sync; `null` if it failed (the scheduler retries).
    #[schema(value_type = Option<SyncReportDoc>)]
    sync: Option<SyncReport>,
}

#[derive(Serialize, ToSchema)]
pub struct ProjectDetail {
    #[serde(flatten)]
    project: ProjectView,
    /// Latest daily cost estimate, `null` before the first inventory cycle.
    cost_estimate: Option<CostView>,
}

#[derive(Serialize, ToSchema)]
pub struct ProjectDetailEnvelope {
    project: ProjectDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ProjectSynced {
    #[schema(value_type = SyncReportDoc)]
    sync: SyncReport,
    project: Option<ProjectView>,
}

#[derive(Serialize, ToSchema)]
pub struct ResourceList {
    resources: Vec<ResourceView>,
}

/// List the organisation's projects. Scope: `read`.
#[utoipa::path(get, path = "/projects", operation_id = "listProjects", tag = "projects",
    responses((status = OK, body = ProjectList)))]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<ProjectList>, ApiError> {
    principal.require(Scope::Read)?;
    let projects = repo::projects::list(&state.db, principal.org).await?;
    Ok(Json(ProjectList { projects }))
}

/// Connect a Hetzner Cloud project. Scope: `write`.
///
/// Validates the token against Hetzner, stores it encrypted, creates the default rules and runs the first sync.
#[utoipa::path(post, path = "/projects", operation_id = "createProject", tag = "projects",
    request_body = CreateProject,
    responses(
        (status = CREATED, body = ProjectCreated),
        (status = BAD_REQUEST, description = "Invalid body (`bad_request`)", body = ErrorBody),
        (status = CONFLICT, description = "Name taken (`name_taken`)", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "Hetzner rejected the token (`invalid_token`)", body = ErrorBody),
        (status = BAD_GATEWAY, description = "Hetzner unavailable (`hetzner_unavailable`)", body = ErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "Hetzner rate limit (`hetzner_rate_limited`)", body = ErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateProject>, JsonRejection>,
) -> Result<(StatusCode, Json<ProjectCreated>), ApiError> {
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
    Ok((StatusCode::CREATED, Json(ProjectCreated { project, sync })))
}

/// A project with its schedule, budget and latest cost estimate. Scope: `read`.
#[utoipa::path(get, path = "/projects/{id}", operation_id = "getProject", tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = OK, body = ProjectDetailEnvelope),
        (status = NOT_FOUND, description = "Unknown project (`not_found`)", body = ErrorBody),
    ))]
pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ProjectDetailEnvelope>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let project = repo::projects::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let cost_estimate = repo::snapshots::latest_cost(&state.db, principal.org, id).await?;
    Ok(Json(ProjectDetailEnvelope {
        project: ProjectDetail {
            project,
            cost_estimate,
        },
    }))
}

/// Disconnect a project and delete its data. Scope: `write`.
#[utoipa::path(delete, path = "/projects/{id}", operation_id = "deleteProject", tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = NO_CONTENT, description = "Deleted"),
        (status = NOT_FOUND, description = "Unknown project (`not_found`)", body = ErrorBody),
    ))]
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

/// Re-sync the inventory now (any project status). Scope: `write`.
#[utoipa::path(post, path = "/projects/{id}/sync", operation_id = "syncProject", tag = "projects",
    params(("id" = Uuid, Path, description = "Project id")),
    responses(
        (status = OK, body = ProjectSynced),
        (status = NOT_FOUND, description = "Unknown project (`not_found`)", body = ErrorBody),
        (status = CONFLICT, description = "A poll cycle is running (`sync_in_progress`)", body = ErrorBody),
        (status = BAD_GATEWAY, description = "Hetzner rejected the token or is unavailable \
            (`hetzner_invalid_token`, `hetzner_unavailable`)", body = ErrorBody),
        (status = SERVICE_UNAVAILABLE, description = "Hetzner rate limit (`hetzner_rate_limited`)", body = ErrorBody),
    ))]
pub async fn sync(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<ProjectSynced>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let report = inventory::sync(&state, principal.org, id).await?;
    let project = repo::projects::get(&state.db, principal.org, id).await?;
    Ok(Json(ProjectSynced {
        sync: report,
        project,
    }))
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ResourceQuery {
    /// Only resources of this kind.
    #[param(value_type = Option<crate::openapi::enums::ResourceKind>)]
    kind: Option<String>,
}

/// The project's inventory (including resources deleted at Hetzner, with `deleted_at`). Scope: `read`.
#[utoipa::path(get, path = "/projects/{id}/resources", operation_id = "listResources", tag = "projects",
    params(("id" = Uuid, Path, description = "Project id"), ResourceQuery),
    responses(
        (status = OK, body = ResourceList),
        (status = BAD_REQUEST, description = "Unknown `kind` (`bad_request`)", body = ErrorBody),
        (status = NOT_FOUND, description = "Unknown project (`not_found`)", body = ErrorBody),
    ))]
pub async fn resources(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
    Query(query): Query<ResourceQuery>,
) -> Result<Json<ResourceList>, ApiError> {
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
    Ok(Json(ResourceList { resources }))
}
