use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, Query, State};
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
use crate::repo::rules::{Change, Patch};

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
    Ok(Json(json!({ "rule": rule })))
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

    let name = body.name.map(|n| n.trim().to_owned());
    if name
        .as_ref()
        .is_some_and(|n| n.is_empty() || n.chars().count() > 200)
    {
        return Err(ApiError::bad_request("`name` must be 1–200 characters"));
    }
    if let Some(params) = &body.params {
        check
            .validate_params(params)
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
    }
    if let Some(severity) = &body.severity
        && !matches!(severity.as_str(), "info" | "warning" | "critical")
    {
        return Err(ApiError::bad_request(
            "`severity` must be info, warning or critical",
        ));
    }
    let selector = match body.selector {
        Nullable::Value(raw) if !raw.trim().is_empty() => {
            if raw.len() > 1000 {
                return Err(ApiError::bad_request(
                    "`selector` must be ≤ 1000 characters",
                ));
            }
            let parsed: Selector = raw.parse().map_err(|e: hm_core::selector::SelectorError| {
                ApiError::bad_request(e.to_string())
            })?;
            Change::Set(parsed.to_string())
        }
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
    tx.commit().await?;
    Ok(Json(json!({ "rule": rule })))
}
