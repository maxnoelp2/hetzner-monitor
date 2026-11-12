//! Integration tests for the health endpoints and the migrations.
//!
//! `#[sqlx::test]` creates a fresh database per test from `DATABASE_URL` and applies
//! `/migrations`. Start Postgres first, e.g. `docker compose -f deploy/docker-compose.yml up -d postgres`.

mod common;

use axum::http::StatusCode;
use sqlx::PgPool;

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn healthz_is_ok(pool: PgPool) {
    let (status, body) = common::call(
        &common::state(pool, "http://127.0.0.1:9"),
        "GET",
        "/healthz",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn readyz_is_ok_when_database_is_reachable(pool: PgPool) {
    let (status, body) = common::call(
        &common::state(pool, "http://127.0.0.1:9"),
        "GET",
        "/readyz",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn readyz_is_unavailable_when_database_is_gone(pool: PgPool) {
    pool.close().await;
    let (status, body) = common::call(
        &common::state(pool, "http://127.0.0.1:9"),
        "GET",
        "/readyz",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "unavailable");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn healthz_does_not_depend_on_the_database(pool: PgPool) {
    pool.close().await;
    let (status, _) = common::call(
        &common::state(pool, "http://127.0.0.1:9"),
        "GET",
        "/healthz",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn migrations_create_all_tables(pool: PgPool) {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name <> '_sqlx_migrations' ORDER BY table_name",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        tables,
        [
            "alert_events",
            "alerts",
            "api_keys",
            "cost_estimates",
            "hetzner_projects",
            "memberships",
            "notification_channels",
            "notification_jobs",
            "organizations",
            "resources",
            "rule_channels",
            "rules",
            "sessions",
            "silences",
            "traffic_samples",
            "users"
        ]
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn membership_role_and_email_are_constrained(pool: PgPool) {
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
    let duplicate = sqlx::query("INSERT INTO users (email) VALUES ('admin@example.com')")
        .execute(&pool)
        .await;
    assert!(
        duplicate.is_err(),
        "e-mail must be unique case-insensitively"
    );
}
