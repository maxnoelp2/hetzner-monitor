//! `/auth/login`, `/auth/logout`, `/me`, `/me/password` (ADR-0007, ADR-0013).

use std::net::SocketAddr;
use std::time::Instant;

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::header::{RETRY_AFTER, SET_COOKIE, USER_AGENT};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use hm_core::{password, session};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::bad_json;
use crate::auth::{Credential, Principal, Scope};
use crate::error::ApiError;
use crate::http::AppState;
use crate::repo::{self, OrgId, UserId};

/// Longest e-mail address accepted at login (RFC 5321 path limit).
const MAX_EMAIL_LEN: usize = 320;

/// The client's address from the listener, if the server was started with connect info.
pub struct PeerAddr(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|c| c.0);
        std::future::ready(Ok(Self(addr)))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Login {
    email: String,
    password: String,
    /// Log into this org instead of the user's oldest membership.
    org_id: Option<Uuid>,
}

fn invalid_credentials() -> ApiError {
    ApiError::new(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "e-mail or password is wrong",
    )
}

/// `Set-Cookie` for the session; `Secure` unless `HM_BASE_URL` is plain `http://`.
fn session_cookie(state: &AppState, value: &str, max_age_s: i64) -> HeaderValue {
    let secure = state
        .base_url
        .as_deref()
        .is_none_or(|b| !b.starts_with("http://"));
    let cookie = format!(
        "{}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age_s}{}",
        session::COOKIE,
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&cookie).expect("cookie is ASCII")
}

async fn profile(
    state: &AppState,
    org: OrgId,
    user: UserId,
    credential: &Credential,
) -> Result<Value, ApiError> {
    let me = repo::users::me(&state.db, org, user)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    let (auth, csrf) = match credential {
        Credential::ApiKey(_) => ("api_key", None),
        Credential::Session { csrf, .. } => ("session", Some(csrf.expose().as_str())),
    };
    Ok(json!({
        "user": { "id": me.user_id, "email": me.email, "name": me.name },
        "org": { "id": me.org_id, "name": me.org_name },
        "role": me.role,
        "auth": auth,
        "csrf_token": csrf,
    }))
}

/// Runs Argon2 off the async runtime.
async fn verify_password(password: String, stored: Option<String>) -> Result<bool, ApiError> {
    tokio::task::spawn_blocking(move || {
        if let Some(stored) = stored {
            password::verify(&password, &stored)
        } else {
            password::verify_dummy(&password);
            false
        }
    })
    .await
    .map_err(|e| ApiError::internal(&e))
}

pub async fn login(
    State(state): State<AppState>,
    PeerAddr(peer): PeerAddr,
    headers: HeaderMap,
    body: Result<Json<Login>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    let email = body.email.trim().to_owned();
    if email.is_empty() || email.len() > MAX_EMAIL_LEN || body.password.is_empty() {
        return Err(invalid_credentials());
    }
    let addr = peer.map(|p| p.ip().to_string());
    if let Some(wait) = state
        .login_throttle
        .check(&email, addr.as_deref(), Instant::now())
    {
        let mut response = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            "too many failed logins; try again later",
        )
        .into_response();
        response
            .headers_mut()
            .insert(RETRY_AFTER, HeaderValue::from(wait.as_secs().max(1)));
        return Ok(response);
    }

    let candidate = repo::users::find_for_login(&state.db, &email, body.org_id).await?;
    let (stored, target) = match candidate {
        Some(c) if !c.disabled => match c.org_id {
            Some(org) => (c.password_hash, Some((OrgId(org), UserId(c.user_id)))),
            None => (None, None),
        },
        _ => (None, None),
    };
    let ok = verify_password(body.password, stored).await?;
    let Some((org, user)) = target.filter(|_| ok) else {
        state
            .login_throttle
            .fail(&email, addr.as_deref(), Instant::now());
        tracing::info!(peer = addr.as_deref().unwrap_or("-"), "login failed");
        return Err(invalid_credentials());
    };
    state.login_throttle.succeed(&email);

    repo::sessions::purge_expired(&state.db).await?;
    let token = session::generate().map_err(|e| ApiError::internal(&e))?;
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(300).collect::<String>());
    let id =
        repo::sessions::insert(&state.db, org, user, &token.hash, user_agent.as_deref()).await?;
    tracing::info!(session = %id, user = %user.0, org = %org.0, "login");
    let raw = token.token.expose().as_str();
    let credential = Credential::Session {
        id,
        csrf: session::csrf_token(raw),
    };
    let body = profile(&state, org, user, &credential).await?;
    let max_age = i64::from(repo::sessions::MAX_AGE_DAYS) * 86_400;
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, session_cookie(&state, raw, max_age));
    Ok(response)
}

pub async fn logout(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Response, ApiError> {
    if let Credential::Session { id, .. } = principal.credential {
        repo::sessions::delete(&state.db, principal.org, id).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response
        .headers_mut()
        .insert(SET_COOKIE, session_cookie(&state, "", 0));
    Ok(response)
}

pub async fn me(
    State(state): State<AppState>,
    principal: Principal,
) -> Result<Json<Value>, ApiError> {
    principal.require(Scope::Read)?;
    Ok(Json(
        profile(&state, principal.org, principal.user, &principal.credential).await?,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePassword {
    current_password: Option<String>,
    new_password: String,
}

/// Changes the caller's password and ends the user's other sessions. A user without a password
/// (bootstrapped before M5) can set one only with an `admin` API key.
pub async fn change_password(
    State(state): State<AppState>,
    principal: Principal,
    body: Result<Json<ChangePassword>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    principal.require(Scope::Read)?;
    let Json(body) = body.map_err(|r| bad_json(&r))?;
    password::validate(&body.new_password).map_err(|e| ApiError::bad_request(e.to_string()))?;
    let stored = repo::users::password_hash(&state.db, principal.org, principal.user).await?;
    if let Some(stored) = stored {
        let current = body.current_password.unwrap_or_default();
        if !verify_password(current, Some(stored)).await? {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "wrong_password",
                "`current_password` is wrong",
            ));
        }
    } else {
        if !matches!(principal.credential, Credential::ApiKey(_)) {
            return Err(ApiError::unauthorized());
        }
        principal.require(Scope::Admin)?;
    }
    let new = body.new_password;
    let hash = tokio::task::spawn_blocking(move || password::hash(&new))
        .await
        .map_err(|e| ApiError::internal(&e))?
        .map_err(|e| ApiError::internal(&e))?;
    let keep = match principal.credential {
        Credential::Session { id, .. } => Some(id),
        Credential::ApiKey(_) => None,
    };
    let mut tx = state.db.begin().await?;
    repo::users::set_password_hash(&mut *tx, principal.user, &hash).await?;
    let ended = repo::sessions::delete_for_user(&mut *tx, principal.user, keep).await?;
    tx.commit().await?;
    tracing::info!(user = %principal.user.0, ended_sessions = ended, "password changed");
    Ok(StatusCode::NO_CONTENT)
}
