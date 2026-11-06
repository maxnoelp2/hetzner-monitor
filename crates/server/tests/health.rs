//! Integration tests for the health endpoints and the initial migration.
//!
//! `#[sqlx::test]` creates a fresh database per test from `DATABASE_URL` and applies
//! `/migrations`. Start Postgres first, e.g. `docker compose -f deploy/docker-compose.yml up -d postgres`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hm_server::http::{AppState, router};
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

async fn get(pool: PgPool, path: &str) -> (StatusCode, Value) {
    let response = router(AppState { db: pool })
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn healthz_is_ok(pool: PgPool) {
    let (status, body) = get(pool, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn readyz_is_ok_when_database_is_reachable(pool: PgPool) {
    let (status, body) = get(pool, "/readyz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn readyz_is_unavailable_when_database_is_gone(pool: PgPool) {
    pool.close().await;
    let (status, body) = get(pool, "/readyz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "unavailable");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn healthz_does_not_depend_on_the_database(pool: PgPool) {
    pool.close().await;
    let (status, _) = get(pool, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn initial_migration_creates_tenancy_tables(pool: PgPool) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name IN ('organizations', 'users', 'memberships') \
         ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(tables, ["memberships", "organizations", "users"]);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn membership_role_is_constrained(pool: PgPool) {
    let org: String = sqlx::query_scalar(
        "INSERT INTO organizations (name) VALUES ('Default') RETURNING id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let user: String = sqlx::query_scalar(
        "INSERT INTO users (email) VALUES ('Admin@Example.com') RETURNING id::text",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let invalid = sqlx::query(
        "INSERT INTO memberships (org_id, user_id, role) VALUES ($1::uuid, $2::uuid, 'superuser')",
    )
    .bind(org)
    .bind(user)
    .execute(&pool)
    .await;
    assert!(
        invalid.is_err(),
        "role outside owner|admin|member must be rejected"
    );

    // citext: e-mail uniqueness is case-insensitive.
    let duplicate = sqlx::query("INSERT INTO users (email) VALUES ('admin@example.com')")
        .execute(&pool)
        .await;
    assert!(
        duplicate.is_err(),
        "e-mail must be unique case-insensitively"
    );
}
