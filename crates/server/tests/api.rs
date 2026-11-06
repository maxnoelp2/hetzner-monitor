//! API integration tests: auth, bootstrap, connect, sync, tenant isolation.
//! Hetzner is mocked with wiremock; Postgres is real (`DATABASE_URL`, see health.rs).

mod common;

use axum::http::StatusCode;
use common::{TOKEN, call, mock_inventory, mock_unauthorized, org_with_key, state};
use hm_core::crypto::{self, MasterKey, Sealed};
use hm_server::auth::Scope;
use hm_server::bootstrap;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::MockServer;

async fn connect(
    st: &hm_server::http::AppState,
    key: &str,
    name: &str,
) -> (StatusCode, serde_json::Value) {
    call(
        st,
        "POST",
        "/api/v1/projects",
        Some(key),
        Some(json!({ "name": name, "token": TOKEN })),
    )
    .await
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn auth_matrix(pool: PgPool) {
    let st = state(pool.clone(), "http://127.0.0.1:9");
    let (_, admin) = org_with_key(&pool, "A", Scope::Admin).await;
    let (_, reader) = org_with_key(&pool, "B", Scope::Read).await;

    assert_eq!(
        call(&st, "GET", "/api/v1/projects", None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some("garbage"), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let wrong_secret = format!("{}x", &admin[..admin.len() - 1]);
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some(&wrong_secret), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some(&admin), None)
            .await
            .0,
        StatusCode::OK
    );

    // read scope: may read, may not write or manage keys
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some(&reader), None)
            .await
            .0,
        StatusCode::OK
    );
    let (status, body) = connect(&st, &reader, "x").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "forbidden");
    assert_eq!(
        call(&st, "GET", "/api/v1/api-keys", Some(&reader), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    // create, use, revoke a key
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/api-keys",
        Some(&admin),
        Some(json!({ "name": "ci", "scopes": ["read"] })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let new_key = body["key"].as_str().unwrap().to_owned();
    let new_id = body["api_key"]["id"].as_str().unwrap().to_owned();
    assert!(body["api_key"].get("key_hash").is_none());
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some(&new_key), None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &st,
            "DELETE",
            &format!("/api/v1/api-keys/{new_id}"),
            Some(&admin),
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&st, "GET", "/api/v1/projects", Some(&new_key), None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );

    let (status, _) = call(
        &st,
        "POST",
        "/api/v1/api-keys",
        Some(&admin),
        Some(json!({ "name": "x", "scopes": ["root"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn bootstrap_runs_once(pool: PgPool) {
    let done = bootstrap::run(&pool, "Default", "admin@example.com")
        .await
        .unwrap();
    let st = state(pool.clone(), "http://127.0.0.1:9");
    let key = done.key.full.expose().as_str().to_owned();
    let (status, body) = call(&st, "GET", "/api/v1/api-keys", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["api_keys"][0]["scopes"], json!(["admin"]));

    let again = bootstrap::run(&pool, "Other", "other@example.com").await;
    assert!(matches!(
        again,
        Err(bootstrap::BootstrapError::AlreadyBootstrapped)
    ));
    assert!(matches!(
        bootstrap::run(&pool, " ", "a@b").await,
        Err(bootstrap::BootstrapError::Invalid(_))
    ));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn connect_stores_sealed_token_and_syncs(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_inventory(&hetzner, &[(1, "web-1"), (2, "web-2")]).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "A", Scope::Write).await;

    let (status, body) = connect(&st, &key, "prod").await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let project = &body["project"];
    assert_eq!(project["status"], "active");
    assert_eq!(project["token_hint"], "ABCD");
    assert_eq!(project["rl_limit"], 3600);
    assert_eq!(project["rl_remaining"], 3500);
    assert!(project["last_inventory_at"].is_string());
    assert_eq!(
        body["sync"]["server"],
        json!({ "seen": 2, "new": 2, "deleted": 0 })
    );
    assert!(
        !body.to_string().contains(TOKEN),
        "token must never be returned"
    );

    // At rest: ciphertext only, and it opens to the token with the row id as AAD.
    let id: Uuid = project["id"].as_str().unwrap().parse().unwrap();
    let (secret_ct, dek_ct, key_version): (Vec<u8>, Vec<u8>, i32) = sqlx::query_as(
        "SELECT token_ct, token_dek_ct, key_version FROM hetzner_projects WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        !secret_ct
            .windows(TOKEN.len())
            .any(|w| w == TOKEN.as_bytes())
    );
    let master = MasterKey::from_base64(common::TEST_MASTER_KEY).unwrap();
    let sealed = Sealed {
        secret_ct,
        dek_ct,
        key_version,
    };
    assert_eq!(
        crypto::open(&master, id.as_bytes(), &sealed)
            .unwrap()
            .as_slice(),
        TOKEN.as_bytes()
    );

    let (status, body) = call(
        &st,
        "GET",
        &format!("/api/v1/projects/{id}/resources?kind=server"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["web-1", "web-2"]);
    assert_eq!(body["resources"][0]["labels"]["env"], "test");
    assert!(body["resources"][0].get("snapshot").is_none());

    let (status, _) = call(
        &st,
        "GET",
        &format!("/api/v1/projects/{id}/resources?kind=router"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(connect(&st, &key, "prod").await.0, StatusCode::CONFLICT);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn invalid_token_is_rejected_and_nothing_is_stored(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_unauthorized(&hetzner).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "A", Scope::Write).await;

    let (status, body) = connect(&st, &key, "prod").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "invalid_token");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM hetzner_projects")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn resync_tracks_new_deleted_and_revived(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_inventory(&hetzner, &[(1, "a"), (2, "b")]).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "A", Scope::Write).await;
    let (_, body) = connect(&st, &key, "p").await;
    let id = body["project"]["id"].as_str().unwrap().to_owned();
    let sync = format!("/api/v1/projects/{id}/sync");

    mock_inventory(&hetzner, &[(2, "b-renamed"), (3, "c")]).await;
    let (status, body) = call(&st, "POST", &sync, Some(&key), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["sync"]["server"],
        json!({ "seen": 2, "new": 1, "deleted": 1 })
    );

    mock_inventory(&hetzner, &[(1, "a"), (2, "b-renamed"), (3, "c")]).await;
    let (_, body) = call(&st, "POST", &sync, Some(&key), None).await;
    assert_eq!(
        body["sync"]["server"],
        json!({ "seen": 3, "new": 0, "deleted": 0 })
    );

    let (_, body) = call(
        &st,
        "GET",
        &format!("/api/v1/projects/{id}/resources"),
        Some(&key),
        None,
    )
    .await;
    let rows = body["resources"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(
        rows.iter().all(|r| r["deleted_at"].is_null()),
        "server 1 must be revived"
    );
    assert!(rows.iter().any(|r| r["name"] == "b-renamed"));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn revoked_hetzner_token_marks_project_invalid(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_inventory(&hetzner, &[(1, "a")]).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "A", Scope::Write).await;
    let (_, body) = connect(&st, &key, "p").await;
    let id = body["project"]["id"].as_str().unwrap().to_owned();

    mock_unauthorized(&hetzner).await;
    let (status, body) = call(
        &st,
        "POST",
        &format!("/api/v1/projects/{id}/sync"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["error"]["code"], "hetzner_invalid_token");
    let (_, body) = call(
        &st,
        "GET",
        &format!("/api/v1/projects/{id}"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(body["project"]["status"], "invalid_token");
    // Inventory from the last good sync is kept.
    let (_, body) = call(
        &st,
        "GET",
        &format!("/api/v1/projects/{id}/resources"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(body["resources"][0]["deleted_at"], serde_json::Value::Null);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn tenants_are_isolated(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_inventory(&hetzner, &[(1, "a")]).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key_a) = org_with_key(&pool, "A", Scope::Admin).await;
    let (_, key_b) = org_with_key(&pool, "B", Scope::Admin).await;
    let (_, body) = connect(&st, &key_a, "secret-project").await;
    let id = body["project"]["id"].as_str().unwrap().to_owned();

    let (_, list) = call(&st, "GET", "/api/v1/projects", Some(&key_b), None).await;
    assert_eq!(list["projects"], json!([]));
    for (method, uri) in [
        ("GET", format!("/api/v1/projects/{id}")),
        ("GET", format!("/api/v1/projects/{id}/resources")),
        ("POST", format!("/api/v1/projects/{id}/sync")),
        ("DELETE", format!("/api/v1/projects/{id}")),
    ] {
        let (status, _) = call(&st, method, &uri, Some(&key_b), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    // Org B cannot see or revoke org A's keys either.
    let (_, keys_a) = call(&st, "GET", "/api/v1/api-keys", Some(&key_a), None).await;
    let key_a_id = keys_a["api_keys"][0]["id"].as_str().unwrap().to_owned();
    let (_, keys_b) = call(&st, "GET", "/api/v1/api-keys", Some(&key_b), None).await;
    assert_eq!(keys_b["api_keys"].as_array().unwrap().len(), 1);
    assert_eq!(
        call(
            &st,
            "DELETE",
            &format!("/api/v1/api-keys/{key_a_id}"),
            Some(&key_b),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Project still there for A.
    assert_eq!(
        call(
            &st,
            "GET",
            &format!("/api/v1/projects/{id}"),
            Some(&key_a),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}
