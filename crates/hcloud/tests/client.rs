//! Client behaviour against a mocked Hetzner API. Fixtures are hand-written and anonymised,
//! shaped after the `OpenAPI` spec (2026-11-05).

use hm_hcloud::{Client, Error, ResourceKind, Token};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn token() -> Token {
    Token::new("test-token-0000".to_owned())
}

fn page(items: &serde_json::Value, page: u32, next: Option<u32>) -> serde_json::Value {
    json!({
        "servers": items,
        "meta": { "pagination": { "page": page, "per_page": 50, "previous_page": null,
                                   "next_page": next, "last_page": 2, "total_entries": 3 } }
    })
}

fn client(server: &MockServer) -> Client {
    Client::with_base_url(&server.uri()).unwrap()
}

#[tokio::test]
async fn lists_all_pages_with_auth_and_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("page", "1"))
        .and(query_param("per_page", "50"))
        .and(header("authorization", "Bearer test-token-0000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            &json!([
                { "id": 101, "name": "web-1", "labels": { "env": "prod" }, "status": "running" },
                { "id": 102, "name": "web-2", "labels": {}, "status": "off" }
            ]),
            1,
            Some(2),
        )))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/servers"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("RateLimit-Limit", "3600")
                .insert_header("RateLimit-Remaining", "3597")
                .insert_header("RateLimit-Reset", "1793000000")
                .set_body_json(page(&json!([{ "id": 103, "name": "db-1" }]), 2, None)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let listed = client(&server)
        .list(&token(), ResourceKind::Server)
        .await
        .unwrap();
    let ids: Vec<i64> = listed.items.iter().map(|r| r.id).collect();
    assert_eq!(ids, [101, 102, 103]);
    assert_eq!(listed.items[0].labels["env"], "prod");
    assert_eq!(listed.items[0].raw["status"], "running");
    assert!(listed.items[2].labels.is_empty());
    assert_eq!(listed.rate_limit.limit, Some(3600));
    assert_eq!(listed.rate_limit.remaining, Some(3597));
    assert_eq!(listed.rate_limit.reset_at, Some(1_793_000_000));
}

#[tokio::test]
async fn uses_the_collection_of_each_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/primary_ips"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "primary_ips": [{ "id": 7, "name": "ip-1", "labels": {} }],
            "meta": { "pagination": { "next_page": null } }
        })))
        .mount(&server)
        .await;
    let listed = client(&server)
        .list(&token(), ResourceKind::PrimaryIp)
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 1);
}

async fn error_for(status: u16, body: serde_json::Value, headers: &[(&str, &str)]) -> Error {
    let server = MockServer::start().await;
    let mut template = ResponseTemplate::new(status).set_body_json(body);
    for (k, v) in headers {
        template = template.insert_header(*k, *v);
    }
    Mock::given(method("GET"))
        .respond_with(template)
        .mount(&server)
        .await;
    client(&server).validate_token(&token()).await.unwrap_err()
}

#[tokio::test]
async fn maps_api_errors() {
    let err = |code: &str, message: &str| json!({ "error": { "code": code, "message": message } });

    assert_eq!(
        error_for(401, err("unauthorized", "unable to authenticate"), &[]).await,
        Error::InvalidToken
    );
    assert!(matches!(
        error_for(403, err("maintenance", "under maintenance"), &[]).await,
        Error::Maintenance { .. }
    ));
    assert!(matches!(
        error_for(403, err("forbidden", "insufficient permissions"), &[]).await,
        Error::Forbidden { .. }
    ));
    assert_eq!(
        error_for(
            429,
            err("rate_limit_exceeded", "limit"),
            &[("RateLimit-Reset", "1793000123")]
        )
        .await,
        Error::RateLimited {
            reset_at: Some(1_793_000_123)
        }
    );
    assert!(matches!(
        error_for(503, json!({}), &[]).await,
        Error::Unavailable {
            status: Some(503),
            ..
        }
    ));
    assert!(matches!(
        error_for(404, err("not_found", "nope"), &[]).await,
        Error::Api { status: 404, .. }
    ));
}

#[tokio::test]
async fn rejects_malformed_responses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/volumes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "volumes": [{ "name": "no-id" }] })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/certificates"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    let c = client(&server);
    assert!(matches!(
        c.list(&token(), ResourceKind::Volume).await,
        Err(Error::Decode(_))
    ));
    assert!(matches!(
        c.list(&token(), ResourceKind::Certificate).await,
        Err(Error::Decode(_))
    ));
}

#[tokio::test]
async fn connection_errors_are_unavailable_and_never_contain_the_token() {
    // Nothing listens on port 9 (discard) on loopback in CI or locally.
    let c = Client::with_base_url("http://127.0.0.1:9").unwrap();
    let err = c.validate_token(&token()).await.unwrap_err();
    assert!(matches!(err, Error::Unavailable { status: None, .. }));
    assert!(!format!("{err} {err:?}").contains("test-token-0000"));
}

/// Live check of TLS and error mapping against the real API with a fake token.
/// Run explicitly: `cargo test -p hm-hcloud -- --ignored live_`.
#[tokio::test]
#[ignore = "needs network access to api.hetzner.cloud"]
async fn live_api_rejects_a_fake_token() {
    let err = Client::new()
        .unwrap()
        .validate_token(&token())
        .await
        .unwrap_err();
    assert_eq!(err, Error::InvalidToken);
}
