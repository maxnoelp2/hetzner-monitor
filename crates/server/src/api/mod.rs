//! `/api/v1` routes.

mod alerts;
mod api_keys;
mod auth;
mod channels;
mod projects;
mod rules;
mod silences;

use axum::extract::rejection::{JsonRejection, PathRejection};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::error::ApiError;
use crate::http::AppState;

/// `/api/v1` routes. Register routes only with `routes!` so they appear in the OpenAPI document.
pub fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(auth::login))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(auth::change_password))
        .routes(routes!(projects::list, projects::create))
        .routes(routes!(projects::get, projects::remove))
        .routes(routes!(projects::sync))
        .routes(routes!(projects::resources))
        .routes(routes!(rules::list, rules::create))
        .routes(routes!(rules::get, rules::update, rules::remove))
        .routes(routes!(alerts::list))
        .routes(routes!(alerts::get))
        .routes(routes!(alerts::ack))
        .routes(routes!(channels::list, channels::create))
        .routes(routes!(channels::get, channels::update, channels::remove))
        .routes(routes!(channels::test))
        .routes(routes!(silences::list, silences::create))
        .routes(routes!(silences::remove))
        .routes(routes!(api_keys::list, api_keys::create))
        .routes(routes!(api_keys::revoke))
}

/// Maps a JSON body rejection to our error shape.
fn bad_json(rejection: &JsonRejection) -> ApiError {
    ApiError::bad_request(rejection.body_text())
}

/// Unparseable ids are treated like unknown ids.
fn bad_path(_: &PathRejection) -> ApiError {
    ApiError::not_found()
}
