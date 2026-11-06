//! API key authentication (ADR-0007). `Authorization: Bearer hm_<prefix>_<secret>`.

use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use hm_core::apikey;
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

/// The authenticated caller.
#[derive(Debug, Clone)]
pub struct Principal {
    pub org: OrgId,
    pub user: UserId,
    pub key_id: Uuid,
    /// Highest scope of the key.
    pub scope: Scope,
}

impl Principal {
    /// Fails with 403 unless the key has at least `needed`.
    pub fn require(&self, needed: Scope) -> Result<(), ApiError> {
        if self.scope >= needed {
            Ok(())
        } else {
            Err(ApiError::forbidden(needed.as_str()))
        }
    }
}

impl FromRequestParts<AppState> for Principal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let presented = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or_else(ApiError::unauthorized)?;
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
        Ok(Self {
            org: OrgId(key.org_id),
            user: UserId(key.user_id),
            key_id: key.id,
            scope,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Scope;

    #[test]
    fn scopes_are_ordered() {
        assert!(Scope::Admin > Scope::Write && Scope::Write > Scope::Read);
        for s in [Scope::Read, Scope::Write, Scope::Admin] {
            assert_eq!(Scope::parse(s.as_str()), Some(s));
        }
        assert_eq!(Scope::parse("root"), None);
    }
}
