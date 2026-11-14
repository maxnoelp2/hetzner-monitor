//! HTTP server setup: shared state, router, health endpoints. See `docs/design/api.md`.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json, Router};
use hm_core::crypto::Keyring;
use serde::Serialize;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

/// How long `/readyz` waits for the database before reporting unavailable.
const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

/// Shared state for all handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub keyring: Arc<Keyring>,
    pub hcloud: hm_hcloud::Client,
    /// Sends notifications (channel tests from the API, queued jobs from the notifier).
    pub notifier: hm_notify::Notifier,
    /// `HM_BASE_URL` without trailing slash, for links in notifications and the cookie's `Secure` flag.
    pub base_url: Option<Arc<str>>,
    /// Failed-login counters (in-process).
    pub login_throttle: Arc<crate::throttle::LoginThrottle>,
    /// Web Push: VAPID subject and the cached key pair (ADR-0017).
    pub push: crate::push::PushState,
}

/// All routes with their OpenAPI description. Routes are registered **only** here and in
/// [`crate::api::routes`], always through `routes!`, so none can be missing from the document
/// (`tests/openapi.rs` fails on plain Axum routing here).
fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(<crate::openapi::ApiDoc as utoipa::OpenApi>::openapi())
        .routes(routes!(healthz))
        .routes(routes!(readyz))
        .routes(routes!(openapi_json))
        .nest("/api/v1", crate::api::routes())
}

/// The OpenAPI 3.1 document of this server (also printed by `hetzner-monitor openapi`).
pub fn openapi() -> utoipa::openapi::OpenApi {
    let (_, mut doc) = routes().split_for_parts();
    crate::openapi::finish(&mut doc);
    doc
}

/// Builds the application router.
pub fn router(state: AppState) -> Router {
    let (router, mut doc) = routes().split_for_parts();
    crate::openapi::finish(&mut doc);
    let doc = Arc::new(doc);
    router
        // Everything no route matched: the web UI, or the JSON 404 under `/api` (ADR-0015, `web.rs`).
        .fallback(crate::web::fallback)
        .layer(Extension(doc))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// `{"status": …}` of the health endpoints.
#[derive(Serialize, ToSchema)]
pub struct Health {
    /// `ok` or `unavailable`.
    status: &'static str,
    /// Why the service is not ready (`database`).
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

/// Liveness: the process is up and serving requests. Never touches the database.
#[utoipa::path(get, path = "/healthz", operation_id = "healthz", tag = "system",
    responses((status = OK, description = "The process is up", body = Health)))]
async fn healthz() -> Json<Health> {
    Json(Health {
        status: "ok",
        reason: None,
    })
}

/// The OpenAPI 3.1 document of this API.
#[utoipa::path(get, path = "/api/v1/openapi.json", operation_id = "openapi", tag = "system",
    responses((status = OK, description = "OpenAPI 3.1 document", content_type = "application/json",
        body = Object)))]
async fn openapi_json(
    Extension(doc): Extension<Arc<utoipa::openapi::OpenApi>>,
) -> Json<Arc<utoipa::openapi::OpenApi>> {
    Json(doc)
}

/// Readiness: the service can reach its database.
#[utoipa::path(get, path = "/readyz", operation_id = "readyz", tag = "system",
    responses(
        (status = OK, description = "The database is reachable", body = Health),
        (status = SERVICE_UNAVAILABLE, description = "The database is not reachable", body = Health),
    ))]
async fn readyz(State(state): State<AppState>) -> (StatusCode, Json<Health>) {
    let check = tokio::time::timeout(
        READINESS_TIMEOUT,
        sqlx::query("SELECT 1").execute(&state.db),
    )
    .await;
    match check {
        Ok(Ok(_)) => (
            StatusCode::OK,
            Json(Health {
                status: "ok",
                reason: None,
            }),
        ),
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

fn unavailable() -> (StatusCode, Json<Health>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(Health {
            status: "unavailable",
            reason: Some("database"),
        }),
    )
}
