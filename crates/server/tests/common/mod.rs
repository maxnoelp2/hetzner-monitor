//! Shared helpers for integration tests: app state with a mocked Hetzner API, keys, requests.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hm_core::apikey;
use hm_core::crypto::MasterKey;
use hm_server::auth::Scope;
use hm_server::http::{AppState, router};
use hm_server::repo::{self, OrgId, UserId};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Fixed test-only master key (32 bytes 0x00..0x1f).
pub const TEST_MASTER_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
/// Fake Hetzner token used against the mock.
pub const TOKEN: &str = "fake-hcloud-token-ABCD";

pub fn state(db: PgPool, hcloud_base: &str) -> AppState {
    AppState {
        db,
        master_key: Arc::new(MasterKey::from_base64(TEST_MASTER_KEY).unwrap()),
        hcloud: hm_hcloud::Client::with_base_url(hcloud_base).unwrap(),
    }
}

/// An org with one owner and an API key of the given scope. Returns (org id, full key).
pub async fn org_with_key(db: &PgPool, name: &str, scope: Scope) -> (Uuid, String) {
    let org: Uuid = sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(db)
        .await
        .unwrap();
    let user: Uuid = sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(format!("{}@example.com", Uuid::new_v4()))
        .fetch_one(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO memberships (org_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(org)
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    let key = apikey::generate().unwrap();
    repo::api_keys::insert(db, OrgId(org), UserId(user), "test", &[scope], &key)
        .await
        .unwrap();
    (org, key.full.expose().as_str().to_owned())
}

/// Sends a request to the router. `key` becomes a Bearer header; `body` is sent as JSON.
pub async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    key: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(key) = key {
        req = req.header("authorization", format!("Bearer {key}"));
    }
    let req = match body {
        Some(body) => req
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let response = router(state.clone()).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

fn ok_json(body: Value) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("RateLimit-Limit", "3600")
        .insert_header("RateLimit-Remaining", "3500")
        .insert_header("RateLimit-Reset", "1793000000")
        .set_body_json(body)
}

fn listing(collection: &str, items: &Value) -> Value {
    json!({ collection: items, "meta": { "pagination": { "page": 1, "per_page": 50, "next_page": null } } })
}

/// Mounts a Hetzner project containing the given servers (id, name) and nothing else.
/// Resets previous mocks so tests can change the inventory between syncs.
pub async fn mock_inventory(server: &MockServer, servers: &[(i64, &str)]) {
    server.reset().await;
    let items: Vec<Value> = servers
        .iter()
        .map(|(id, name)| json!({ "id": id, "name": name, "labels": { "env": "test" }, "status": "running" }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("per_page", "1"))
        .respond_with(ok_json(listing(
            "servers",
            &json!(items.iter().take(1).collect::<Vec<_>>()),
        )))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("per_page", "50"))
        .respond_with(ok_json(listing("servers", &json!(items))))
        .mount(server)
        .await;
    for collection in [
        "load_balancers",
        "volumes",
        "certificates",
        "floating_ips",
        "primary_ips",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{collection}")))
            .respond_with(ok_json(listing(collection, &json!([]))))
            .mount(server)
            .await;
    }
}

/// Makes every Hetzner request fail with 401.
pub async fn mock_unauthorized(server: &MockServer) {
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({ "error": { "code": "unauthorized", "message": "unable to authenticate" } }),
        ))
        .mount(server)
        .await;
}

/// A server object as Hetzner returns it (only the fields the checks read).
pub fn server_json(id: i64, name: &str, status: &str) -> Value {
    json!({
        "id": id, "name": name, "status": status, "labels": { "env": "test" },
        "outgoing_traffic": 0, "ingoing_traffic": 0, "included_traffic": 20_000_000_000_000_u64
    })
}

/// Mounts a Hetzner project with the given server and load balancer objects.
/// Resets previous mocks so tests can change the project between cycles.
pub async fn mock_project(server: &MockServer, servers: &[Value], load_balancers: &[Value]) {
    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("per_page", "1"))
        .respond_with(ok_json(listing(
            "servers",
            &json!(servers.iter().take(1).collect::<Vec<_>>()),
        )))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("per_page", "50"))
        .respond_with(ok_json(listing("servers", &json!(servers))))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/load_balancers"))
        .respond_with(ok_json(listing("load_balancers", &json!(load_balancers))))
        .mount(server)
        .await;
    for collection in ["volumes", "certificates", "floating_ips", "primary_ips"] {
        Mock::given(method("GET"))
            .and(path(format!("/{collection}")))
            .respond_with(ok_json(listing(collection, &json!([]))))
            .mount(server)
            .await;
    }
}

/// Makes every Hetzner request fail with `status` (and optional rate-limit reset header).
pub async fn mock_failure(server: &MockServer, status: u16, code: &str, reset_at: Option<i64>) {
    server.reset().await;
    let mut response = ResponseTemplate::new(status)
        .set_body_json(json!({ "error": { "code": code, "message": "mocked failure" } }));
    if let Some(reset) = reset_at {
        response = response
            .insert_header("RateLimit-Limit", "3600")
            .insert_header("RateLimit-Remaining", "0")
            .insert_header("RateLimit-Reset", reset.to_string().as_str());
    }
    Mock::given(method("GET"))
        .respond_with(response)
        .mount(server)
        .await;
}
