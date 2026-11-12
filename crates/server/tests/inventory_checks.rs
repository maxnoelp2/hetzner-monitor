//! M3b: inventory, hygiene and cost checks and the traffic forecast against a mocked Hetzner API.
//! Fixtures are hand-written after the `OpenAPI` spec (2026-11-12) and anonymised.

mod common;

use axum::http::StatusCode;
use common::{call, mock_kinds, org_with_key, server_json, state};
use hm_server::auth::Scope;
use hm_server::http::AppState;
use hm_server::scheduler;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use wiremock::MockServer;

const DAY: i64 = 86_400;

struct Env {
    st: AppState,
    hetzner: MockServer,
    key: String,
    project: String,
    t0: OffsetDateTime,
}

impl Env {
    async fn new(pool: PgPool, kinds: &[(&str, Vec<Value>)]) -> Self {
        let hetzner = MockServer::start().await;
        mock_kinds(&hetzner, kinds).await;
        let st = state(pool.clone(), &hetzner.uri());
        let (_, key) = org_with_key(&pool, "acme", Scope::Admin).await;
        let t0 = OffsetDateTime::now_utc();
        let (status, body) = call(
            &st,
            "POST",
            "/api/v1/projects",
            Some(&key),
            Some(json!({ "name": "prod", "token": common::TOKEN })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let project = body["project"]["id"].as_str().unwrap().to_owned();
        Self {
            st,
            hetzner,
            key,
            project,
            t0,
        }
    }

    /// Runs the scheduler `secs` seconds after connect; returns the number of cycles.
    async fn at(&self, secs: i64) -> usize {
        scheduler::tick(&self.st, self.t0 + Duration::seconds(secs))
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str) -> Value {
        let (status, body) = call(&self.st, "GET", uri, Some(&self.key), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        body
    }

    /// `(state, message)` of the alerts of `check_type` that are not resolved, sorted.
    async fn open(&self, check_type: &str) -> Vec<(String, String)> {
        self.with_state(check_type, "open").await
    }

    async fn with_state(&self, check_type: &str, state: &str) -> Vec<(String, String)> {
        let body = self
            .get(&format!(
                "/api/v1/alerts?project_id={}&state={state}&per_page=100",
                self.project
            ))
            .await;
        let mut alerts: Vec<(String, String)> = body["alerts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["check_type"] == check_type)
            .map(|a| {
                (
                    a["state"].as_str().unwrap().to_owned(),
                    a["message"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        alerts.sort();
        alerts
    }

    async fn project(&self) -> Value {
        self.get(&format!("/api/v1/projects/{}", self.project))
            .await["project"]
            .clone()
    }
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap()
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// A server without backups and protection, plus an unattached volume, an unassigned primary
/// IP, a certificate whose renewal failed and a load balancer of a deprecated type.
fn messy_project(cert_expiry: OffsetDateTime, renewal: &str) -> Vec<(&'static str, Vec<Value>)> {
    let mut server = server_json(1, "web-1", "running");
    server["backup_window"] = Value::Null;
    server["protection"] = json!({ "delete": false, "rebuild": true });
    vec![
        ("servers", vec![server]),
        (
            "load_balancers",
            vec![json!({
                "id": 3, "name": "lb-1", "labels": {}, "targets": [], "location": { "name": "fsn1" },
                "load_balancer_type": { "name": "lb11", "prices": [], "deprecation": {
                    "announced": "2026-09-01T00:00:00Z", "unavailable_after": "2027-03-01T00:00:00Z" } },
                "protection": { "delete": true }, "outgoing_traffic": 0, "included_traffic": 20_000_000_000_000_u64
            })],
        ),
        (
            "volumes",
            vec![
                json!({ "id": 4, "name": "data", "labels": {}, "server": null, "size": 10,
                        "status": "available", "location": { "name": "fsn1" }, "protection": { "delete": false } }),
            ],
        ),
        (
            "primary_ips",
            vec![
                json!({ "id": 9, "name": "spare", "labels": {}, "ip": "198.51.100.9", "type": "ipv4",
                        "assignee_id": null, "assignee_type": "server", "location": { "name": "fsn1" },
                        "protection": { "delete": false } }),
            ],
        ),
        (
            "certificates",
            vec![json!({
                "id": 5, "name": "api", "labels": {}, "type": "managed", "domain_names": ["example.com"],
                "not_valid_after": rfc3339(cert_expiry),
                "status": { "issuance": "completed", "renewal": renewal,
                            "error": if renewal == "failed" { json!({ "code": "dns_validation_failed", "message": "DNS record not found" }) } else { Value::Null } }
            })],
        ),
    ]
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn inventory_checks_run_on_inventory_cycles_only(pool: PgPool) {
    let t0 = OffsetDateTime::now_utc();
    let expiry = t0 + Duration::days(40) + Duration::hours(12);
    let env = Env::new(pool, &messy_project(expiry, "failed")).await;
    let date = expiry.date().to_string();

    // Connect is an inventory cycle: zero-hold checks fire, hygiene checks wait for `for_s`.
    assert_eq!(
        env.open("certificate_expiry").await,
        pairs(&[(
            "firing",
            &format!(
                "certificate api (5): renewal failed (DNS record not found); expires in 40 days ({date})"
            )
        )])
    );
    assert_eq!(
        env.open("deprecation").await,
        pairs(&[(
            "firing",
            "load balancer lb-1 (3): load balancer type lb11 is deprecated, unavailable after 2027-03-01 (announced 2026-09-01)"
        )])
    );
    assert_eq!(
        env.open("hygiene_backups").await,
        pairs(&[("pending", "server web-1 (1): backups disabled")])
    );
    assert_eq!(
        env.open("hygiene_protection").await,
        pairs(&[("pending", "server web-1 (1): delete protection off")])
    );
    assert_eq!(
        env.open("hygiene_orphans").await,
        pairs(&[
            (
                "pending",
                "primary ip spare (9) [198.51.100.9]: not assigned to a server"
            ),
            ("pending", "volume data (4): not attached to a server"),
        ])
    );

    // The certificate is renewed; a fast cycle does not look at certificates.
    let fixed = messy_project(t0 + Duration::days(90), "scheduled");
    mock_kinds(&env.hetzner, &fixed).await;
    assert_eq!(env.at(66).await, 1);
    assert_eq!(env.open("certificate_expiry").await.len(), 1);
    // Inventory cycles (every 15 min): resolved after two good cycles.
    env.at(15 * 60 + 6).await;
    assert_eq!(env.open("certificate_expiry").await.len(), 1);
    env.at(30 * 60 + 12).await;
    assert_eq!(env.open("certificate_expiry").await, []);
    assert_eq!(
        env.with_state("certificate_expiry", "resolved").await.len(),
        1
    );

    // After an hour the hygiene findings fire; orphans need a day.
    env.at(61 * 60).await;
    assert_eq!(env.open("hygiene_backups").await[0].0, "firing");
    assert_eq!(env.open("hygiene_protection").await[0].0, "firing");
    assert!(
        env.open("hygiene_orphans")
            .await
            .iter()
            .all(|(s, _)| s == "pending")
    );
    env.at(DAY + 120).await;
    assert!(
        env.open("hygiene_orphans")
            .await
            .iter()
            .all(|(s, _)| s == "firing")
    );
    assert_eq!(env.open("hygiene_orphans").await.len(), 2);

    // Disabling a hygiene rule closes its alerts.
    let rules = env
        .get(&format!("/api/v1/rules?project_id={}", env.project))
        .await;
    let id = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check_type"] == "hygiene_orphans")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, _) = call(
        &env.st,
        "PATCH",
        &format!("/api/v1/rules/{id}"),
        Some(&env.key),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(env.open("hygiene_orphans").await, []);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn protection_kinds_are_configurable(pool: PgPool) {
    let t0 = OffsetDateTime::now_utc();
    let env = Env::new(pool, &messy_project(t0 + Duration::days(90), "scheduled")).await;
    let rules = env
        .get(&format!("/api/v1/rules?project_id={}", env.project))
        .await;
    let id = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check_type"] == "hygiene_protection")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let uri = format!("/api/v1/rules/{id}");
    let patch = |params: Value| {
        call(
            &env.st,
            "PATCH",
            &uri,
            Some(&env.key),
            Some(json!({ "params": params })),
        )
    };
    let (status, body) = patch(json!({ "kinds": ["certificate"] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = patch(json!({ "kinds": ["volume", "primary_ip"], "for_s": 0 })).await;
    assert_eq!(status, StatusCode::OK);
    env.at(15 * 60 + 6).await;
    // The server is no longer in scope: its pending alert is dropped.
    assert_eq!(
        env.open("hygiene_protection").await,
        pairs(&[
            ("firing", "primary ip spare (9): delete protection off"),
            ("firing", "volume data (4): delete protection off"),
        ])
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn cost_change_compares_with_an_earlier_day(pool: PgPool) {
    let one = vec![("servers", vec![server_json(1, "web-1", "running")])];
    let env = Env::new(pool, &one).await;
    // cx22 in fsn1 = 4.00, +20 % backups (on in `server_json`).
    let cost = env.project().await["cost_estimate"].clone();
    assert_eq!(cost["monthly_net"], 4.8, "{cost}");
    assert_eq!(cost["currency"], "EUR");
    assert_eq!(cost["unpriced"], 0);
    assert_eq!(cost["day"], env.t0.date().to_string());
    assert_eq!(env.open("cost_change").await, []);

    // Next day: a bigger server joins and a volume is attached (10 GB × 0.05).
    let mut big = server_json(2, "db-1", "running");
    big["server_type"]["name"] = json!("cx42");
    let more = vec![
        ("servers", vec![server_json(1, "web-1", "running"), big]),
        (
            "volumes",
            vec![
                json!({ "id": 4, "name": "data", "labels": {}, "server": 2, "size": 10, "status": "available",
                        "location": { "name": "fsn1" }, "protection": { "delete": true } }),
            ],
        ),
    ];
    mock_kinds(&env.hetzner, &more).await;
    env.at(DAY + 60).await;
    let day0 = env.t0.date().to_string();
    // 4.8 + 19.2 + 0.5 = 24.5 → +410.4 %
    assert_eq!(
        env.open("cost_change").await,
        pairs(&[(
            "firing",
            &format!(
                "estimated monthly cost 24.50 EUR, +410.4 % vs. {day0} (4.80 EUR); net, without traffic overage and snapshots"
            )
        )])
    );
    assert_eq!(env.project().await["cost_estimate"]["monthly_net"], 24.5);

    // The day after, the new level is the baseline: resolved after two inventory cycles.
    env.at(2 * DAY + 60).await;
    env.at(2 * DAY + 16 * 60).await;
    assert_eq!(env.open("cost_change").await, []);
    assert_eq!(env.with_state("cost_change", "resolved").await.len(), 1);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn traffic_forecast_extrapolates_from_a_day_old_sample(pool: PgPool) {
    let tb = 1_000_000_000_000_u64;
    let server = |out: u64| {
        let mut s = server_json(1, "web-1", "running");
        s["included_traffic"] = json!(tb);
        s["outgoing_traffic"] = json!(out);
        vec![("servers", vec![s])]
    };
    let env = Env::new(pool, &server(0)).await;
    let forecast = |alerts: Vec<(String, String)>| {
        alerts
            .into_iter()
            .filter(|(_, m)| m.contains("forecast"))
            .collect::<Vec<_>>()
    };
    assert_eq!(forecast(env.open("traffic_quota").await), []);

    // 700 GB in a day: the month's quota is gone well before its end. Evaluate at least six
    // hours before a month ends, so the extrapolation has room even on the last day.
    let mut at = DAY + 60;
    let month_end_close = |secs: i64| {
        let t = env.t0 + Duration::seconds(secs) + Duration::hours(6);
        t.month() != (env.t0 + Duration::seconds(secs)).month()
    };
    if month_end_close(at) {
        at += DAY / 2;
    }
    mock_kinds(&env.hetzner, &server(tb * 7 / 10)).await;
    env.at(at).await;
    let open = forecast(env.open("traffic_quota").await);
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(open[0].0, "firing");
    assert!(
        open[0]
            .1
            .contains("estimate, assumes the quota resets on the 1st, UTC"),
        "{open:?}"
    );
    // The current-use rules see 70 %: no 80 % or 100 % alert.
    assert_eq!(env.open("traffic_quota").await.len(), 1);
}
