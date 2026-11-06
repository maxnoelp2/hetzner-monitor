//! HTTP API (Axum). M0 exposes only health endpoints. See `docs/design/api.md`.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

/// How long `/readyz` waits for the database before reporting unavailable.
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state for all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
}

/// Builds the application router.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Liveness: the process is up and serving requests. Never touches the database.
async fn healthz() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// Readiness: the service can reach its database.
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let check = tokio::time::timeout(
        READINESS_TIMEOUT,
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await;
    match check {
        Ok(Ok(_)) => (StatusCode::OK, Json(json!({ "status": "ok" }))),
        Ok(Err(error)) => {
            tracing::warn!(%error, "readiness check failed: database error");
            unavailable()
        }
        Err(_) => {
            tracing::warn!(
                timeout_s = READINESS_TIMEOUT.as_secs(),
                "readiness check failed: timeout"
            );
            unavailable()
        }
    }
}

fn unavailable() -> (StatusCode, Json<Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "status": "unavailable", "reason": "database" })),
    )
}
