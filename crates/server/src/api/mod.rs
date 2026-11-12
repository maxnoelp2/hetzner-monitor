//! `/api/v1` routes.

mod alerts;
mod api_keys;
mod auth;
mod channels;
mod projects;
mod rules;
mod silences;

use axum::Router;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::routing::{delete, get, post, put};

use crate::error::ApiError;
use crate::http::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route("/me", get(auth::me))
        .route("/me/password", put(auth::change_password))
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
        .route("/alerts/{id}/ack", post(alerts::ack))
        .route("/channels", get(channels::list).post(channels::create))
        .route(
            "/channels/{id}",
            get(channels::get)
                .patch(channels::update)
                .delete(channels::remove),
        )
        .route("/channels/{id}/test", post(channels::test))
        .route("/silences", get(silences::list).post(silences::create))
        .route("/silences/{id}", delete(silences::remove))
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
