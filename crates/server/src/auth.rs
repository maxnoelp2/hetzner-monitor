//! Authentication (ADR-0007, ADR-0013): `Authorization: Bearer hm_<prefix>_<secret>` API keys,
//! or the `hm_session` cookie with an `X-CSRF-Token` header on state-changing requests.

use axum::extract::FromRequestParts;
use axum::http::Method;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use hm_core::secret::Secret;
use hm_core::{apikey, session};
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::AppState;
use crate::repo::{self, OrgId, UserId};

/// API key scopes. `admin` implies `write` implies `read`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Read,
    Write,
    Admin,
}

impl Scope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "write" => Some(Self::Write),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// How the caller authenticated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    ApiKey(Uuid),
    /// Session id and its CSRF token (handed to the client by `/me`).
    Session {
        id: Uuid,
        csrf: Secret<String>,
    },
}

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub org: OrgId,
    pub user: UserId,
    pub credential: Credential,
    /// Highest scope of the key, or the scope of the session's role.
    pub scope: Scope,
}

impl Principal {
    /// Fails with 403 unless the caller has at least `needed`.
    pub fn require(&self, needed: Scope) -> Result<(), ApiError> {
        if self.scope >= needed {
            Ok(())
        } else {
            Err(ApiError::forbidden(needed.as_str()))
        }
    }
}

/// Scope of a session by membership role: `owner`/`admin` → admin, `member` → write.
#[must_use]
pub fn scope_for_role(role: &str) -> Option<Scope> {
    match role {
        "owner" | "admin" => Some(Scope::Admin),
        "member" => Some(Scope::Write),
        _ => None,
    }
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // An Authorization header always wins and never falls back to the cookie.
        if let Some(value) = parts.headers.get(AUTHORIZATION) {
            let presented = value
                .to_str()
                .ok()
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::trim)
                .ok_or_else(ApiError::unauthorized)?;
            return from_api_key(state, presented).await;
        }
        let token = parts
            .headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find_map(|v| session::cookie_value(v, session::COOKIE))
            .filter(|t| session::well_formed(t))
            .ok_or_else(ApiError::unauthorized)?;
        let found = repo::sessions::find_valid(&state.db, &session::hash(token))
            .await?
            .ok_or_else(ApiError::unauthorized)?;
        if !is_safe(&parts.method) {
            let csrf_ok = parts
                .headers
                .get(session::CSRF_HEADER)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|presented| session::verify_csrf(presented.trim(), token));
            if !csrf_ok {
                return Err(ApiError::csrf_failed());
            }
        }
        let scope = scope_for_role(&found.role).ok_or_else(ApiError::unauthorized)?;
        repo::sessions::touch(&state.db, found.id).await?;
        Ok(Self {
            org: OrgId(found.org_id),
            user: UserId(found.user_id),
            credential: Credential::Session {
                id: found.id,
                csrf: session::csrf_token(token),
            },
            scope,
        })
    }
}

fn is_safe(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

async fn from_api_key(state: &AppState, presented: &str) -> Result<Principal, ApiError> {
    let prefix = apikey::parse_prefix(presented).ok_or_else(ApiError::unauthorized)?;
    let key = repo::api_keys::find_active_by_prefix(&state.db, prefix)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
    if !apikey::verify(presented, &key.key_hash) {
        return Err(ApiError::unauthorized());
    }
    let scope = key
        .scopes
        .iter()
        .filter_map(|s| Scope::parse(s))
        .max()
        .ok_or_else(ApiError::unauthorized)?;
    repo::api_keys::touch(&state.db, key.id).await?;
    Ok(Principal {
        org: OrgId(key.org_id),
        user: UserId(key.user_id),
        credential: Credential::ApiKey(key.id),
        scope,
    })
}

#[cfg(test)]
mod tests {
    use super::{Scope, scope_for_role};

    #[test]
    fn scopes_are_ordered() {
        assert!(Scope::Admin > Scope::Write && Scope::Write > Scope::Read);
        for s in [Scope::Read, Scope::Write, Scope::Admin] {
            assert_eq!(Scope::parse(s.as_str()), Some(s));
        }
        assert_eq!(Scope::parse("root"), None);
    }

    #[test]
    fn session_scope_follows_role() {
        assert_eq!(scope_for_role("owner"), Some(Scope::Admin));
        assert_eq!(scope_for_role("admin"), Some(Scope::Admin));
        assert_eq!(scope_for_role("member"), Some(Scope::Write));
        assert_eq!(scope_for_role("guest"), None);
    }
}
