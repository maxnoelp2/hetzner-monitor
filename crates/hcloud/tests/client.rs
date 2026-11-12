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

#[tokio::test]
async fn fetches_and_parses_server_metrics() {
    use hm_hcloud::MetricType;
    use time::macros::datetime;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/servers/42/metrics"))
        .and(query_param("type", "cpu,network"))
        .and(query_param("start", "2026-11-11T10:00:00Z"))
        .and(query_param("end", "2026-11-11T10:10:00Z"))
        .and(query_param("step", "60"))
        .and(header("authorization", "Bearer test-token-0000"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("RateLimit-Limit", "3600")
                .insert_header("RateLimit-Remaining", "3590")
                .insert_header("RateLimit-Reset", "1793000000")
                .set_body_json(json!({ "metrics": {
                    "start": "2026-11-11T10:00:00Z", "end": "2026-11-11T10:10:00Z", "step": 60,
                    "time_series": {
                        "cpu": { "values": [[1_794_391_260.0, "12.5"], [1_794_391_200.0, "10"], [1_794_391_320.0, "NaN"], [1_794_391_380.0, "x"]] },
                        "network.0.bandwidth.in": { "values": [[1_794_391_200.0, "1024"]] }
                    }
                }})),
        )
        .mount(&server)
        .await;
    let metrics = client(&server)
        .server_metrics(
            &token(),
            42,
            &[MetricType::Cpu, MetricType::Network],
            datetime!(2026-11-11 10:00 UTC),
            datetime!(2026-11-11 10:10 UTC),
            60,
        )
        .await
        .unwrap();
    assert!((metrics.step - 60.0).abs() < f64::EPSILON);
    // Sorted by time; NaN and non-numeric samples dropped.
    assert_eq!(
        metrics.series["cpu"],
        [(1_794_391_200.0, 10.0), (1_794_391_260.0, 12.5)]
    );
    assert_eq!(
        metrics.series["network.0.bandwidth.in"],
        [(1_794_391_200.0, 1024.0)]
    );
    assert_eq!(metrics.rate_limit.remaining, Some(3590));
}

#[tokio::test]
async fn metrics_of_a_vanished_server_is_a_404_api_error() {
    use hm_hcloud::MetricType;
    use time::macros::datetime;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/servers/7/metrics"))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({ "error": { "code": "not_found", "message": "server with ID '7' not found" } }),
        ))
        .mount(&server)
        .await;
    let err = client(&server)
        .server_metrics(
            &token(),
            7,
            &[MetricType::Cpu],
            datetime!(2026-11-11 10:00 UTC),
            datetime!(2026-11-11 10:10 UTC),
            60,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Api { status: 404, .. }), "{err:?}");
}

#[tokio::test]
async fn pricing_returns_the_raw_pricing_object() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pricing"))
        .and(header("authorization", "Bearer test-token-0000"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("RateLimit-Limit", "3600")
                .insert_header("RateLimit-Remaining", "3580")
                .set_body_json(json!({ "pricing": {
                    "currency": "EUR", "vat_rate": "19.00",
                    "volume": { "price_per_gb_month": { "net": "0.0440000000", "gross": "0.0523600000000000" } },
                    "server_backup": { "percentage": "20.00" }
                }})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let priced = client(&server).pricing(&token()).await.unwrap();
    assert_eq!(priced.raw["currency"], "EUR");
    assert_eq!(priced.raw["server_backup"]["percentage"], "20.00");
    assert_eq!(priced.rate_limit.remaining, Some(3580));

    server.reset().await;
    Mock::given(method("GET"))
        .and(path("/pricing"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "nope": {} })))
        .mount(&server)
        .await;
    let err = client(&server).pricing(&token()).await.unwrap_err();
    assert!(matches!(err, Error::Decode(_)), "{err:?}");
}
