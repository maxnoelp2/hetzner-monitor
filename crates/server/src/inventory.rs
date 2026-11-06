//! Connecting projects and syncing their inventory from Hetzner.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use hm_core::crypto;
use hm_hcloud::{RateLimit, ResourceKind, Token};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::ApiError;
use crate::http::AppState;
use crate::repo::projects::{Inserted, ProjectView};
use crate::repo::resources::KindCounts;
use crate::repo::{self, OrgId};

/// Per-kind counts of one sync, keyed by kind (`server`, …).
pub type SyncReport = BTreeMap<&'static str, KindCounts>;

/// Validates the token, stores the project with the token sealed, and runs the first sync.
/// A failing first sync does not undo the connect; the project keeps the error status.
pub async fn connect(
    state: &AppState,
    org: OrgId,
    name: &str,
    token: Token,
) -> Result<(ProjectView, Option<SyncReport>), ApiError> {
    state
        .hcloud
        .validate_token(&token)
        .await
        .map_err(|err| match err {
            hm_hcloud::Error::InvalidToken => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_token",
                "Hetzner rejected this API token",
            ),
            other => hetzner_error(&other),
        })?;

    let id = Uuid::new_v4();
    let sealed = {
        let plain = token_plaintext(&token);
        crypto::seal(&state.master_key, id.as_bytes(), plain.as_bytes())
            .map_err(|e| ApiError::internal(&e))?
    };
    let project =
        match repo::projects::insert(&state.db, org, id, name, &sealed, &token.hint()).await? {
            Inserted::Ok(project) => project,
            Inserted::NameTaken => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "name_taken",
                    "a project with this name already exists",
                ));
            }
        };
    let report = sync_with_token(state, org, project.id, &token).await.ok();
    let project = repo::projects::get(&state.db, org, project.id)
        .await?
        .unwrap_or(project);
    Ok((project, report))
}

/// Re-syncs a project of this org.
pub async fn sync(state: &AppState, org: OrgId, project_id: Uuid) -> Result<SyncReport, ApiError> {
    let sealed = repo::projects::sealed_token(&state.db, org, project_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let plain = crypto::open(&state.master_key, project_id.as_bytes(), &sealed)
        .map_err(|e| ApiError::internal(&e))?;
    let token = Token::new(String::from_utf8(plain.to_vec()).map_err(|e| ApiError::internal(&e))?);
    sync_with_token(state, org, project_id, &token).await
}

async fn sync_with_token(
    state: &AppState,
    org: OrgId,
    project_id: Uuid,
    token: &Token,
) -> Result<SyncReport, ApiError> {
    let mut fetched = Vec::with_capacity(ResourceKind::ALL.len());
    let mut rate_limit = RateLimit::default();
    for kind in ResourceKind::ALL {
        match state.hcloud.list(token, kind).await {
            Ok(listed) => {
                rate_limit = listed.rate_limit;
                fetched.push((kind, listed.items));
            }
            Err(err) => {
                let status = match err {
                    hm_hcloud::Error::InvalidToken => "invalid_token",
                    _ => "degraded",
                };
                tracing::warn!(project_id = %project_id, kind = %kind, error = %err, "inventory sync failed");
                repo::projects::set_status(
                    &state.db,
                    org,
                    project_id,
                    status,
                    Some(&err.to_string()),
                    None,
                    None,
                )
                .await?;
                return Err(hetzner_error(&err));
            }
        }
    }

    let now = OffsetDateTime::now_utc();
    let mut tx = state.db.begin().await?;
    let mut report = SyncReport::new();
    for (kind, items) in &fetched {
        let counts = repo::resources::apply(&mut tx, project_id, *kind, items, now).await?;
        report.insert(kind.as_str(), counts);
    }
    repo::projects::set_status(
        &mut *tx,
        org,
        project_id,
        "active",
        None,
        Some(rate_limit),
        Some(now),
    )
    .await?;
    tx.commit().await?;
    tracing::info!(project_id = %project_id, "inventory synced");
    Ok(report)
}

fn hetzner_error(err: &hm_hcloud::Error) -> ApiError {
    let (status, code) = match err {
        hm_hcloud::Error::InvalidToken => (StatusCode::BAD_GATEWAY, "hetzner_invalid_token"),
        hm_hcloud::Error::RateLimited { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "hetzner_rate_limited")
        }
        _ => (StatusCode::BAD_GATEWAY, "hetzner_unavailable"),
    };
    ApiError::new(status, code, err.to_string())
}

/// The token's plaintext for sealing. Kept in a zeroizing buffer.
fn token_plaintext(token: &Token) -> zeroize::Zeroizing<String> {
    token.reveal_for_storage()
}
