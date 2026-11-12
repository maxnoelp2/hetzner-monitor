use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use hm_core::checks::CheckType;
use hm_core::selector::Selector;
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::repo;
use crate::repo::rules::{Change, Deleted, NewRule, Patch};

#[derive(Deserialize)]
pub struct ListQuery {
    project_id: Option<Uuid>,
}

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let rules = repo::rules::list(&state.db, principal.org, query.project_id).await?;
    Ok(Json(json!({ "rules": rules })))
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let rule = repo::rules::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let channel_ids = repo::channels::for_rule(&state.db, principal.org, id).await?;
    Ok(Json(json!({ "rule": rule, "channel_ids": channel_ids })))
}

/// `PATCH /rules/{id}`. `selector: null` or `""` clears the selector.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRule {
    name: Option<String>,
    enabled: Option<bool>,
    params: Option<Value>,
    severity: Option<String>,
    #[serde(default)]
    selector: Nullable,
    /// Pinned channels; `[]` unpins all (the rule then routes by severity).
    channel_ids: Option<Vec<Uuid>>,
}

/// A JSON field that distinguishes "absent" from `null`.
#[derive(Default)]
enum Nullable {
    #[default]
    Absent,
    Null,
    Value(String),
}

impl<'de> Deserialize<'de> for Nullable {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        Option::<String>::deserialize(de).map(|v| v.map_or(Self::Null, Self::Value))
    }
}

pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<UpdateRule>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;

    let mut tx = state.db.begin().await?;
    let current = repo::rules::get(&mut *tx, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let check: CheckType = current
        .check_type
        .parse()
        .map_err(|e: String| ApiError::internal(&e))?;

    let name = body.name.as_deref().map(valid_name).transpose()?;
    if let Some(params) = &body.params {
        check
            .validate_params(params)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
    if let Some(severity) = &body.severity {
        valid_severity(severity)?;
    }
    let selector = match body.selector {
        Nullable::Value(raw) if !raw.trim().is_empty() => Change::Set(valid_selector(&raw)?),
        Nullable::Value(_) | Nullable::Null => Change::Clear,
        Nullable::Absent => Change::Keep,
    };

    let now = OffsetDateTime::now_utc();
    let patch = Patch {
        name,
        enabled: body.enabled,
        params: body.params,
        severity: body.severity,
        selector,
    };
    let rule = repo::rules::update(&mut tx, principal.org, id, &patch, now)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !rule.enabled && current.enabled {
        repo::alerts::close_for_rule(&mut tx, principal.org, id, "rule disabled", now).await?;
    }
    if let Some(ids) = &body.channel_ids
        && !repo::channels::set_for_rule(&mut tx, principal.org, id, ids).await?
    {
        return Err(ApiError::bad_request(
            "`channel_ids` contains a channel that is not part of this organisation",
        ));
    }
    let channel_ids = repo::channels::for_rule(&mut *tx, principal.org, id).await?;
    tx.commit().await?;
    Ok(Json(json!({ "rule": rule, "channel_ids": channel_ids })))
}

/// `POST /rules`: a custom rule. Required: `name`, `check_type`, `severity`; `params` default `{}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRule {
    project_id: Option<Uuid>,
    name: String,
    check_type: String,
    #[serde(default = "empty_object")]
    params: Value,
    severity: String,
    selector: Option<String>,
    resource_id: Option<Uuid>,
    #[serde(default = "yes")]
    enabled: bool,
}

fn empty_object() -> Value {
    json!({})
}

const fn yes() -> bool {
    true
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateRule>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    principal.require(Scope::Write)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let org = principal.org;

    let check: CheckType = body
        .check_type
        .parse()
        .map_err(|e: String| ApiError::bad_request(e))?;
    if check == CheckType::HetznerApi {
        return Err(ApiError::bad_request(
            "`hetzner_api` is a system rule and exists once per project",
        ));
    }
    let name = valid_name(&body.name)?;
    check
        .validate_params(&body.params)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    valid_severity(&body.severity)?;
    let selector = body
        .selector
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(valid_selector)
        .transpose()?;
    if let Some(project_id) = body.project_id
        && !repo::projects::exists(&state.db, org, project_id).await?
    {
        return Err(ApiError::bad_request(
            "`project_id` is not a project of this organisation",
        ));
    }
    if let Some(resource_id) = body.resource_id {
        let Some(project_id) = body.project_id else {
            return Err(ApiError::bad_request("`resource_id` needs `project_id`"));
        };
        if !repo::resources::belongs(&state.db, org, project_id, resource_id).await? {
            return Err(ApiError::bad_request(
                "`resource_id` is not a resource of this project",
            ));
        }
    }

    let rule = repo::rules::insert(
        &state.db,
        org,
        &NewRule {
            project_id: body.project_id,
            name: &name,
            check_type: check.as_str(),
            params: &body.params,
            selector: selector.as_deref(),
            resource_id: body.resource_id,
            severity: &body.severity,
            enabled: body.enabled,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "rule": rule }))))
}

/// `DELETE /rules/{id}`: custom rules only; default rules can be disabled instead.
pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    match repo::rules::delete(&state.db, principal.org, id).await? {
        Deleted::Ok => Ok(StatusCode::NO_CONTENT),
        Deleted::NotFound => Err(ApiError::not_found()),
        Deleted::IsDefault => Err(ApiError::new(
            StatusCode::CONFLICT,
            "default_rule",
            "default rules cannot be deleted; disable them with PATCH {\"enabled\": false}",
        )),
    }
}

fn valid_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(ApiError::bad_request("`name` must be 1–200 characters"));
    }
    Ok(name.to_owned())
}

pub(super) fn valid_severity(severity: &str) -> Result<(), ApiError> {
    if matches!(severity, "info" | "warning" | "critical") {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "`severity` must be info, warning or critical",
        ))
    }
}

/// Parses and normalises a selector.
pub(super) fn valid_selector(raw: &str) -> Result<String, ApiError> {
    if raw.len() > 1000 {
        return Err(ApiError::bad_request(
            "`selector` must be ≤ 1000 characters",
        ));
    }
    raw.parse::<Selector>()
        .map(|s| s.to_string())
        .map_err(|e| ApiError::bad_request(e.to_string()))
}
