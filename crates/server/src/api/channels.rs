//! `/channels`: notification channels. Configs are write-only: they are sealed on input and
//! never returned; responses show only a non-secret `hint`.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hm_core::crypto::{self, Sealed};
use hm_notify::{ChannelConfig, Payload, SendError};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use super::rules::valid_severity;
use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::notifier;
use crate::repo;
use crate::repo::channels::{NewChannel, Patch};

pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let channels = repo::channels::list(&state.db, principal.org).await?;
    Ok(Json(json!({ "channels": channels })))
}

pub async fn get(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let channel = repo::channels::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(Json(json!({ "channel": channel })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateChannel {
    #[serde(rename = "type")]
    kind: String,
    name: String,
    config: Value,
    #[serde(default = "default_min_severity")]
    min_severity: String,
    #[serde(default = "yes")]
    enabled: bool,
}

fn default_min_severity() -> String {
    "warning".to_owned()
}

const fn yes() -> bool {
    true
}

pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreateChannel>, JsonRejection>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    principal.require(Scope::Write)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let name = valid_name(&body.name)?;
    valid_severity(&body.min_severity)?;
    let config = parse_config(&state, &body.kind, &body.config)?;
    let id = Uuid::new_v4();
    let sealed = seal(&state, id, &config)?;
    let hint = config.hint();
    let result = repo::channels::insert(
        &state.db,
        principal.org,
        &NewChannel {
            id,
            kind: config.kind(),
            name: &name,
            sealed: &sealed,
            hint: &hint,
            min_severity: &body.min_severity,
            enabled: body.enabled,
        },
        OffsetDateTime::now_utc(),
    )
    .await;
    let channel = result.map_err(name_conflict)?;
    Ok((StatusCode::CREATED, Json(json!({ "channel": channel }))))
}

/// `PATCH /channels/{id}`. A `config` replaces the whole config (same type).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateChannel {
    name: Option<String>,
    min_severity: Option<String>,
    enabled: Option<bool>,
    config: Option<Value>,
}

pub async fn update(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
    body: Result<Json<UpdateChannel>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let mut tx = state.db.begin().await?;
    let current = repo::channels::get(&mut *tx, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let name = body.name.as_deref().map(valid_name).transpose()?;
    if let Some(s) = &body.min_severity {
        valid_severity(s)?;
    }
    let sealed_hint = body
        .config
        .as_ref()
        .map(|raw| {
            let config = parse_config(&state, &current.kind, raw)?;
            Ok::<_, ApiError>((seal(&state, id, &config)?, config.hint()))
        })
        .transpose()?;
    let patch = Patch {
        name: name.as_deref(),
        min_severity: body.min_severity.as_deref(),
        enabled: body.enabled,
        config: sealed_hint.as_ref().map(|(s, h)| (s, h.as_str())),
    };
    let channel = repo::channels::update(
        &mut tx,
        principal.org,
        id,
        &patch,
        OffsetDateTime::now_utc(),
    )
    .await
    .map_err(name_conflict)?
    .ok_or_else(ApiError::not_found)?;
    tx.commit().await?;
    Ok(Json(json!({ "channel": channel })))
}

pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::channels::delete(&state.db, principal.org, id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// `POST /channels/{id}/test`: sends a test notification now (also to disabled channels).
/// `200 { "ok": true }`, or `200 { "ok": false, "error", "retryable" }` with a redacted reason.
pub async fn test(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let channel = repo::channels::get(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let sealed = repo::channels::sealed_config(&state.db, principal.org, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let now = OffsetDateTime::now_utc();
    let payload = Payload::test(&channel.name, &notifier::rfc3339(now));
    let result = match notifier::decrypt_config(&state, id, &channel.kind, &sealed) {
        Ok(config) => state.notifier.send(&config, &payload, "test").await,
        Err(e) => Err(e),
    };
    tracing::info!(channel_id = %id, ok = result.is_ok(), "channel test");
    Ok(Json(match result {
        Ok(()) => json!({ "ok": true }),
        Err(SendError::Retry { reason, .. }) => {
            json!({ "ok": false, "error": reason, "retryable": true })
        }
        Err(SendError::Permanent { reason }) => {
            json!({ "ok": false, "error": reason, "retryable": false })
        }
    }))
}

fn valid_name(raw: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request("`name` must be 1–100 characters"));
    }
    Ok(name.to_owned())
}

fn parse_config(state: &AppState, kind: &str, raw: &Value) -> Result<ChannelConfig, ApiError> {
    let config = ChannelConfig::from_parts(kind, raw).map_err(|e| ApiError::bad_request(e.0))?;
    if matches!(config, ChannelConfig::Email(_)) && !state.notifier.email_enabled() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "email_disabled",
            "email channels need SMTP settings on the server (HM_SMTP_HOST, HM_SMTP_FROM)",
        ));
    }
    Ok(config)
}

fn seal(state: &AppState, id: Uuid, config: &ChannelConfig) -> Result<Sealed, ApiError> {
    let plain = zeroize::Zeroizing::new(
        serde_json::to_vec(&config.to_json()).map_err(|e| ApiError::internal(&e))?,
    );
    crypto::seal(&state.master_key, id.as_bytes(), &plain).map_err(|e| ApiError::internal(&e))
}

fn name_conflict(err: sqlx::Error) -> ApiError {
    match err {
        sqlx::Error::Database(db) if db.is_unique_violation() => ApiError::new(
            StatusCode::CONFLICT,
            "name_taken",
            "a channel with this name exists",
        ),
        other => other.into(),
    }
}
