//! `/push`: the VAPID public key and the caller's push subscriptions (one per device, ADR-0017).
//! Endpoint and keys are write-only: sealed on input, never returned.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use hm_core::crypto;
use hm_notify::Payload;
use hm_notify::push::Subscription;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{bad_json, bad_path};
use crate::auth::{Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::notifier;
use crate::openapi::ErrorBody;
use crate::push::PushSetupError;
use crate::repo;
use crate::repo::push::{NewSubscription, PushSubscriptionView};

/// The `push_disabled` error (no VAPID subject configured).
pub fn disabled() -> ApiError {
    ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "push_disabled",
        "web push needs HM_VAPID_SUBJECT or an https:// HM_BASE_URL on the server",
    )
}

fn setup_error(e: &PushSetupError) -> ApiError {
    match e {
        PushSetupError::Disabled => disabled(),
        other => ApiError::internal(other),
    }
}

/// Whether Web Push is available, and the key browsers subscribe with.
#[derive(Serialize, ToSchema)]
pub struct PushConfig {
    enabled: bool,
    /// VAPID public key (uncompressed P-256 point, base64url): `applicationServerKey` for
    /// `PushManager.subscribe()`. Absent when push is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    vapid_public_key: Option<String>,
}

/// Web Push availability and the VAPID public key. Scope: `read`.
#[utoipa::path(get, path = "/push/config", operation_id = "getPushConfig", tag = "push",
    responses((status = OK, body = PushConfig)))]
pub async fn config(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<PushConfig>, ApiError> {
    principal.require(Scope::Read)?;
    if !state.push.enabled() {
        return Ok(Json(PushConfig {
            enabled: false,
            vapid_public_key: None,
        }));
    }
    let vapid = state
        .push
        .vapid(&state.db, &state.keyring)
        .await
        .map_err(|e| setup_error(&e))?;
    Ok(Json(PushConfig {
        enabled: true,
        vapid_public_key: Some(vapid.public_key()),
    }))
}

#[derive(Serialize, ToSchema)]
pub struct PushSubscriptionList {
    subscriptions: Vec<PushSubscriptionView>,
}

#[derive(Serialize, ToSchema)]
pub struct PushSubscriptionEnvelope {
    subscription: PushSubscriptionView,
}

/// The caller's subscribed devices in this organisation. Scope: `read`.
#[utoipa::path(get, path = "/push/subscriptions", operation_id = "listPushSubscriptions", tag = "push",
    responses((status = OK, body = PushSubscriptionList)))]
pub async fn list(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<PushSubscriptionList>, ApiError> {
    principal.require(Scope::Read)?;
    let subscriptions = repo::push::list(&state.db, principal.org, principal.user).await?;
    Ok(Json(PushSubscriptionList { subscriptions }))
}

/// The keys of a browser push subscription (`PushSubscription.toJSON().keys`).
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionKeys {
    /// P-256 public key, base64url.
    p256dh: String,
    /// 16-byte authentication secret, base64url.
    auth: String,
}

/// `PushSubscription.toJSON()` plus a device name. Write-only: sealed, never returned.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreatePushSubscription {
    /// The push service URL (`https://`). A secret: whoever knows it can push to the device.
    endpoint: String,
    keys: SubscriptionKeys,
    /// Ignored (part of `PushSubscription.toJSON()`).
    #[serde(default, rename = "expirationTime")]
    #[allow(dead_code)]
    expiration_time: Option<f64>,
    /// 1–100 characters, e.g. "iPhone · Safari". Default "Device".
    name: Option<String>,
}

/// Subscribe this device. Scope: `write`.
///
/// Idempotent per endpoint: registering a known endpoint again replaces its keys and name (and
/// moves it to the caller, if another member of the organisation had registered it) and
/// answers 200.
#[utoipa::path(post, path = "/push/subscriptions", operation_id = "createPushSubscription", tag = "push",
    request_body = CreatePushSubscription,
    responses(
        (status = CREATED, description = "Subscribed", body = PushSubscriptionEnvelope),
        (status = OK, description = "Known endpoint, updated", body = PushSubscriptionEnvelope),
        (status = BAD_REQUEST, description = "Invalid subscription (`bad_request`)", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "Web Push not configured (`push_disabled`)", body = ErrorBody),
    ))]
pub async fn create(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<CreatePushSubscription>, JsonRejection>,
) -> Result<(StatusCode, Json<PushSubscriptionEnvelope>), ApiError> {
    principal.require(Scope::Write)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    if !state.push.enabled() {
        return Err(disabled());
    }
    let sub = Subscription {
        endpoint: body.endpoint.trim().to_owned(),
        p256dh: body.keys.p256dh.trim().to_owned(),
        auth: body.keys.auth.trim().to_owned(),
    };
    sub.validate(false)
        .map_err(|e| ApiError::bad_request(e.0))?;
    let name = match body.name.as_deref().map(str::trim) {
        None | Some("") => "Device".to_owned(),
        Some(n) if n.chars().count() <= 100 => n.to_owned(),
        Some(_) => return Err(ApiError::bad_request("`name` must be 1–100 characters")),
    };
    let plain =
        zeroize::Zeroizing::new(serde_json::to_vec(&sub).map_err(|e| ApiError::internal(&e))?);
    let hash = sub.endpoint_hash();
    let host = sub.host();
    let now = OffsetDateTime::now_utc();
    let mut tx = state.db.begin().await?;
    // Two attempts: an insert can lose a race against a concurrent registration of the endpoint.
    for _ in 0..2 {
        let existing = repo::push::find_by_endpoint(&mut tx, principal.org, &hash).await?;
        let id = existing.unwrap_or_else(Uuid::new_v4);
        let sealed = crypto::seal(&state.keyring, id.as_bytes(), &plain)
            .map_err(|e| ApiError::internal(&e))?;
        let new = NewSubscription {
            id,
            endpoint_hash: &hash,
            sealed: &sealed,
            name: &name,
            host: &host,
        };
        if existing.is_some() {
            let subscription =
                repo::push::replace(&mut tx, principal.org, principal.user, &new, now).await?;
            tx.commit().await?;
            return Ok((
                StatusCode::OK,
                Json(PushSubscriptionEnvelope { subscription }),
            ));
        }
        if let Some(subscription) =
            repo::push::insert(&mut tx, principal.org, principal.user, &new, now).await?
        {
            tx.commit().await?;
            tracing::info!(subscription_id = %id, host, "push subscription created");
            return Ok((
                StatusCode::CREATED,
                Json(PushSubscriptionEnvelope { subscription }),
            ));
        }
    }
    Err(ApiError::internal(
        &"push subscription upsert did not settle",
    ))
}

/// Unsubscribe one of the caller's devices. Scope: `write`.
#[utoipa::path(delete, path = "/push/subscriptions/{id}", operation_id = "deletePushSubscription", tag = "push",
    params(("id" = Uuid, Path, description = "Push subscription id")),
    responses(
        (status = NO_CONTENT, description = "Removed"),
        (status = NOT_FOUND, description = "Unknown subscription (`not_found`)", body = ErrorBody),
    ))]
pub async fn remove(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    if repo::push::delete(
        &state.db,
        principal.org,
        principal.user,
        id,
        OffsetDateTime::now_utc(),
    )
    .await?
    {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

/// Result of a test push. Errors are short and redacted.
#[derive(Serialize, ToSchema)]
pub struct PushTestResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// The push service reported the subscription gone; it was removed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    gone: bool,
}

/// Send a test push to one of the caller's devices. Scope: `write`.
///
/// Delivery failures are reported in the body with status 200.
#[utoipa::path(post, path = "/push/subscriptions/{id}/test", operation_id = "testPushSubscription", tag = "push",
    params(("id" = Uuid, Path, description = "Push subscription id")),
    responses(
        (status = OK, body = PushTestResult),
        (status = NOT_FOUND, description = "Unknown subscription (`not_found`)", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "Web Push not configured (`push_disabled`)", body = ErrorBody),
    ))]
pub async fn test(
    State(state): State<AppState>,
    principal: Principal,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Json<PushTestResult>, ApiError> {
    principal.require(Scope::Write)?;
    let Path(id) = id.map_err(|r| bad_path(&r))?;
    let target = repo::push::own_target(&state.db, principal.org, principal.user, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let vapid = state
        .push
        .vapid(&state.db, &state.keyring)
        .await
        .map_err(|e| setup_error(&e))?;
    let now = OffsetDateTime::now_utc();
    let payload = Payload::test(&target.name, &notifier::rfc3339(now));
    let result = match notifier::decrypt_subscription(&state, id, &target.sealed()) {
        Ok(sub) => state.notifier.send_push(&vapid, &sub, &payload).await,
        Err(e) => Err(e),
    };
    let gone = matches!(result, Err(hm_notify::SendError::Gone { .. }));
    if gone {
        let mut conn = state.db.acquire().await?;
        repo::push::mark_gone(&mut conn, id, now).await?;
    }
    tracing::info!(subscription_id = %id, ok = result.is_ok(), "push test");
    Ok(Json(PushTestResult {
        ok: result.is_ok(),
        error: result.err().map(|e| e.to_string()),
        gone,
    }))
}
