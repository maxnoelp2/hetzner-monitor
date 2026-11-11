//! `/api/v1` routes.

mod alerts;
mod api_keys;
mod projects;
mod rules;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::routing::{delete, get, post};

use crate::error::ApiError;
use crate::http::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/projects", get(projects::list).post(projects::create))
        .route(
            "/projects/{id}",
            get(projects::get).delete(projects::remove),
        )
        .route("/projects/{id}/sync", post(projects::sync))
        .route("/projects/{id}/resources", get(projects::resources))
        .route("/rules", get(rules::list).post(rules::create))
        .route(
            "/rules/{id}",
            get(rules::get).patch(rules::update).delete(rules::remove),
        )
        .route("/alerts", get(alerts::list))
        .route("/alerts/{id}", get(alerts::get))
        .route("/api-keys", get(api_keys::list).post(api_keys::create))
        .route("/api-keys/{id}", delete(api_keys::revoke))
}

/// Maps a JSON body rejection to our error shape.
fn bad_json(rejection: &JsonRejection) -> ApiError {
    ApiError::bad_request(rejection.body_text())
}

/// Unparseable ids are treated like unknown ids.
fn bad_path(_: &PathRejection) -> ApiError {
    ApiError::not_found()
}
