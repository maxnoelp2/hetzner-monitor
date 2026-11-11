//! Connecting projects and syncing them on demand. Both run an inventory [`cycle`].

use axum::http::StatusCode;
use hm_core::crypto;
use hm_hcloud::Token;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::cycle::{self, CycleError, SyncReport, Tier};
use crate::error::ApiError;
use crate::http::AppState;
use crate::repo::projects::{Inserted, ProjectView};
use crate::repo::{self, OrgId};
use crate::scheduler::LEASE;

/// Validates the token, stores the project with the token sealed, creates its default rules,
/// and runs the first sync. A failing first sync does not undo the connect; the project keeps
/// the error status and the scheduler retries.
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
        let plain = token.reveal_for_storage();
        crypto::seal(&state.master_key, id.as_bytes(), plain.as_bytes())
            .map_err(|e| ApiError::internal(&e))?
    };
    let mut tx = state.db.begin().await?;
    let project =
        match repo::projects::insert(&mut *tx, org, id, name, &sealed, &token.hint()).await? {
            Inserted::Ok(project) => *project,
            Inserted::NameTaken => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "name_taken",
                    "a project with this name already exists",
                ));
            }
        };
    repo::rules::create_defaults(&mut *tx, org, project.id).await?;
    tx.commit().await?;

    let now = OffsetDateTime::now_utc();
    let report = match repo::projects::try_lease(&state.db, org, id, now, now + LEASE).await? {
        Some(claimed) => cycle::run(state, &claimed, &token, Tier::Inventory, now)
            .await
            .ok(),
        None => None,
    };
    let project = repo::projects::get(&state.db, org, project.id)
        .await?
        .unwrap_or(project);
    Ok((project, report))
}

/// Re-syncs a project of this org now (inventory tier), regardless of its status.
/// Fails with `409 sync_in_progress` while another cycle holds the project.
pub async fn sync(state: &AppState, org: OrgId, project_id: Uuid) -> Result<SyncReport, ApiError> {
    let now = OffsetDateTime::now_utc();
    let Some(claimed) =
        repo::projects::try_lease(&state.db, org, project_id, now, now + LEASE).await?
    else {
        return Err(
            if repo::projects::exists(&state.db, org, project_id).await? {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "sync_in_progress",
                    "a poll cycle for this project is running, try again in a moment",
                )
            } else {
                ApiError::not_found()
            },
        );
    };
    let result = match cycle::open_token(state, &claimed) {
        Ok(token) => cycle::run(state, &claimed, &token, Tier::Inventory, now).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(report) => Ok(report),
        Err(CycleError::Hetzner(err)) => Err(hetzner_error(&err)),
        Err(err) => {
            let _ = repo::projects::release(&state.db, org, project_id).await;
            Err(ApiError::internal(&err))
        }
    }
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
