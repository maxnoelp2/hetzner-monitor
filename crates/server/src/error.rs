//! API error type: `{ "error": { "code", "message" } }` with an HTTP status (docs/design/api.md).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// An error returned to API clients. Messages must never contain secrets.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing, invalid, expired or revoked credentials",
        )
    }

    pub fn forbidden(scope: &str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("this API key or session lacks the `{scope}` scope"),
        )
    }

    /// A cookie-authenticated, state-changing request without a valid `X-CSRF-Token`.
    pub fn csrf_failed() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "missing or invalid X-CSRF-Token header",
        )
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "resource not found")
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    /// Logs the cause and returns a generic 500 without internal details.
    pub fn internal(error: &dyn std::fmt::Display) -> Self {
        tracing::error!(%error, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal server error",
        )
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::internal(&error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": { "code": self.code, "message": self.message } }));
        (self.status, body).into_response()
    }
}
