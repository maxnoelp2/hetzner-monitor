//! M2: scheduler, rule engine and alert lifecycle against a mocked Hetzner API.
//! Cycles are driven with `scheduler::tick(now)` and a synthetic clock.

mod common;

use axum::http::StatusCode;
use common::{call, mock_failure, mock_project, org_with_key, server_json, state};
use hm_server::auth::Scope;
use hm_server::http::AppState;
use hm_server::scheduler;
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use wiremock::MockServer;

/// Fails with the unexpected items listed.
#[track_caller]
fn assert_none(items: &[Value]) {
    assert_eq!(items, &[] as &[Value], "expected no items");
}

/// A little more than the fast interval plus jitter, so every tick finds the project due.
const STEP: i64 = 66;

struct Env {
    st: AppState,
    hetzner: MockServer,
    key: String,
    project: String,
    t0: OffsetDateTime,
}

impl Env {
    async fn new(pool: PgPool, servers: &[Value]) -> Self {
        Self::with_lbs(pool, servers, &[]).await
    }

    async fn with_lbs(pool: PgPool, servers: &[Value], lbs: &[Value]) -> Self {
        let hetzner = MockServer::start().await;
        mock_project(&hetzner, servers, lbs).await;
        let st = state(pool.clone(), &hetzner.uri());
        let (_, key) = org_with_key(&pool, "acme", Scope::Admin).await;
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
            t0: OffsetDateTime::now_utc(),
        }
    }

    /// Runs the tick `n` steps after `t0` and returns the number of cycles.
    async fn tick(&self, n: i64) -> usize {
        scheduler::tick(&self.st, self.t0 + Duration::seconds(n * STEP))
            .await
            .unwrap()
    }

    async fn get(&self, uri: &str) -> Value {
        let (status, body) = call(&self.st, "GET", uri, Some(&self.key), None).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        body
    }

    async fn alerts(&self, query: &str) -> Vec<Value> {
        self.get(&format!(
            "/api/v1/alerts?project_id={}&{query}",
            self.project
        ))
        .await["alerts"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn alerts_of(&self, check_type: &str, state: &str) -> Vec<Value> {
        self.alerts(&format!("state={state}"))
            .await
            .into_iter()
            .filter(|a| a["check_type"] == check_type)
            .collect()
    }

    async fn project(&self) -> Value {
        self.get(&format!("/api/v1/projects/{}", self.project))
            .await["project"]
            .clone()
    }
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn connect_creates_default_rules_and_schedule(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "running")]).await;
    let rules = env
        .get(&format!("/api/v1/rules?project_id={}", env.project))
        .await;
    let mut types: Vec<&str> = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["check_type"].as_str().unwrap())
        .collect();
    types.sort_unstable();
    assert_eq!(
        types,
        [
            "hetzner_api",
            "lb_target_health",
            "resource_lifecycle",
            "server_status",
            "traffic_quota",
            "traffic_quota"
        ]
    );
    assert!(
        rules["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["is_default"] == true && r["enabled"] == true)
    );

    let project = env.project().await;
    assert!(project["next_fast_at"].is_string());
    assert!(project["next_inventory_at"].is_string());
    assert_eq!(project["budget_h"], 1800);
    // 1 server (1 request/min) + 6 inventory listings every 15 min.
    assert_eq!(project["fixed_h"], 60 + 6 * 4);
    assert_eq!(project["metrics_interval_s"], 60);
    // The first sync is not reported as "created".
    assert_none(&env.alerts("").await);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn power_off_fires_and_power_on_resolves(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "running")]).await;
    assert_eq!(env.tick(1).await, 1);
    assert_none(&env.alerts("").await);

    mock_project(&env.hetzner, &[server_json(1, "web-1", "off")], &[]).await;
    env.tick(2).await;
    let pending = env.alerts_of("server_status", "pending").await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["message"], "server web-1 (1) is off");
    env.tick(3).await; // 66 s < 120 s
    assert_eq!(env.alerts_of("server_status", "pending").await.len(), 1);
    env.tick(4).await; // 132 s ≥ 120 s
    let firing = env.alerts_of("server_status", "firing").await;
    assert_eq!(firing.len(), 1, "{firing:?}");
    assert_eq!(firing[0]["severity"], "critical");
    assert_eq!(firing[0]["resource_name"], "web-1");

    mock_project(&env.hetzner, &[server_json(1, "web-1", "running")], &[]).await;
    env.tick(5).await; // first good cycle: still firing (flapping guard)
    assert_eq!(env.alerts_of("server_status", "firing").await.len(), 1);
    env.tick(6).await;
    assert_none(&env.alerts_of("server_status", "open").await);
    let resolved = env.alerts_of("server_status", "resolved").await;
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["message"], "server web-1 (1) is running");

    let id = resolved[0]["id"].as_str().unwrap();
    let detail = env.get(&format!("/api/v1/alerts/{id}")).await;
    let events: Vec<&str> = detail["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(events, ["pending", "fired", "resolved"]);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn a_short_blip_never_fires(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "running")]).await;
    mock_project(&env.hetzner, &[server_json(1, "web-1", "off")], &[]).await;
    env.tick(1).await;
    assert_eq!(env.alerts_of("server_status", "pending").await.len(), 1);
    mock_project(&env.hetzner, &[server_json(1, "web-1", "running")], &[]).await;
    env.tick(2).await;
    assert_none(&env.alerts("").await);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn api_outage_raises_one_system_alert_and_freezes_server_alerts(pool: PgPool) {
    let servers = [
        server_json(1, "web-1", "off"),
        server_json(2, "web-2", "running"),
    ];
    let env = Env::new(pool, &servers).await;
    for n in 1..=3 {
        env.tick(n).await;
    }
    assert_eq!(env.alerts_of("server_status", "firing").await.len(), 1);

    // Hetzner is down: after 3 failures the project is degraded. Steps of 20 min clear any backoff.
    mock_failure(&env.hetzner, 503, "unavailable", None).await;
    for n in 1..=3 {
        assert_eq!(env.tick(3 + n * 20).await, 1);
        let api = env.alerts_of("hetzner_api", "firing").await;
        assert_eq!(api.len(), usize::from(n == 3), "after failure {n}");
    }
    let project = env.project().await;
    assert_eq!(project["status"], "degraded");
    assert_eq!(project["failures"], 3);
    // No "all servers down": the server alerts are untouched.
    let firing = env.alerts_of("server_status", "firing").await;
    assert_eq!(firing.len(), 1);
    assert_eq!(firing[0]["resource_name"], "web-1");
    assert_none(&env.alerts_of("server_status", "pending").await);

    // Hetzner is back and web-1 runs again: both alerts resolve after two good cycles.
    mock_project(
        &env.hetzner,
        &[
            server_json(1, "web-1", "running"),
            server_json(2, "web-2", "running"),
        ],
        &[],
    )
    .await;
    env.tick(100).await;
    assert_eq!(env.project().await["status"], "active");
    env.tick(101).await;
    assert_none(&env.alerts("state=open").await);
    let api = env.alerts_of("hetzner_api", "resolved").await;
    assert_eq!(api.len(), 1);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn rate_limit_waits_until_reset(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "running")]).await;
    let reset = (env.t0 + Duration::seconds(600)).unix_timestamp();
    mock_failure(&env.hetzner, 429, "rate_limit_exceeded", Some(reset)).await;
    assert_eq!(env.tick(1).await, 1);
    let project = env.project().await;
    assert_eq!(project["status"], "active");
    assert_eq!(project["failures"], 0);
    assert_eq!(project["rl_remaining"], 0);
    // Not due again before the reset (t0 + 600 s), then due.
    assert_eq!(env.tick(8).await, 0);
    mock_project(&env.hetzner, &[server_json(1, "web-1", "running")], &[]).await;
    assert_eq!(env.tick(10).await, 1);
    assert_none(&env.alerts("").await);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn invalid_token_stops_polling(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "running")]).await;
    mock_failure(&env.hetzner, 401, "unauthorized", None).await;
    assert_eq!(env.tick(1).await, 1);
    assert_eq!(env.project().await["status"], "invalid_token");
    let api = env.alerts_of("hetzner_api", "firing").await;
    assert_eq!(api.len(), 1);
    assert!(
        api[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Hetzner rejected the project's API token")
    );
    assert_eq!(env.tick(30).await, 0, "no polling with an invalid token");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn a_project_is_claimed_by_one_worker_only(pool: PgPool) {
    let env = Env::new(pool.clone(), &[server_json(1, "web-1", "running")]).await;
    let now = env.t0 + Duration::seconds(STEP);
    let until = now + scheduler::LEASE;
    let (a, b) = tokio::join!(
        hm_server::repo::projects::claim_due(&pool, now, until, 10),
        hm_server::repo::projects::claim_due(&pool, now, until, 10),
    );
    assert_eq!(a.unwrap().len() + b.unwrap().len(), 1);
    // Leased: neither the scheduler nor a manual sync gets it.
    assert_eq!(scheduler::tick(&env.st, now).await.unwrap(), 0);
    let (status, body) = call(
        &env.st,
        "POST",
        &format!("/api/v1/projects/{}/sync", env.project),
        Some(&env.key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "sync_in_progress");
    // After the lease expires the project is due again.
    assert_eq!(
        scheduler::tick(&env.st, until + Duration::seconds(1))
            .await
            .unwrap(),
        1
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn created_and_deleted_resources_are_reported(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "off")]).await;
    for n in 1..=3 {
        env.tick(n).await;
    }
    assert_eq!(env.alerts_of("server_status", "firing").await.len(), 1);

    mock_project(&env.hetzner, &[server_json(2, "web-2", "running")], &[]).await;
    env.tick(4).await;
    let mut messages: Vec<String> = env
        .alerts_of("resource_lifecycle", "resolved")
        .await
        .iter()
        .map(|a| a["message"].as_str().unwrap().to_owned())
        .collect();
    messages.sort();
    assert_eq!(
        messages,
        ["server web-1 (1) deleted", "server web-2 (2) created"]
    );
    // The deleted server's alert is closed, not left firing.
    assert_none(&env.alerts_of("server_status", "open").await);
    let resolved = env.alerts_of("server_status", "resolved").await;
    assert_eq!(
        resolved[0]["message"],
        "resource deleted or no longer in the rule's scope"
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn traffic_and_load_balancer_checks(pool: PgPool) {
    let mut web = server_json(1, "web-1", "running");
    web["outgoing_traffic"] = json!(17_000_000_000_000_u64); // 85 % of 20 TB
    let lb = json!({
        "id": 7, "name": "lb-1", "labels": {}, "outgoing_traffic": 0, "included_traffic": 20_000_000_000_000_u64,
        "targets": [{ "type": "server", "server": { "id": 1 }, "health_status": [
            { "listen_port": 443, "status": "unhealthy" }
        ]}]
    });
    let env = Env::with_lbs(pool, &[web], &[lb]).await;
    // Checks run on every cycle, the first one included: traffic has no hold time.
    let traffic = env.alerts_of("traffic_quota", "firing").await;
    assert_eq!(traffic.len(), 1, "only the 80 % rule fires: {traffic:?}");
    assert_eq!(traffic[0]["severity"], "warning");
    assert_eq!(traffic[0]["value"], 85.0);
    assert_eq!(env.alerts_of("lb_target_health", "pending").await.len(), 1);
    // The fast tier fetches the load balancer because the project has one.
    env.tick(1).await;
    env.tick(2).await;
    let lb_alerts = env.alerts_of("lb_target_health", "firing").await;
    assert_eq!(lb_alerts.len(), 1);
    assert_eq!(
        lb_alerts[0]["message"],
        "load balancer lb-1 (7): 1 of 1 target services not healthy (server 1:443 unhealthy)"
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn rules_can_be_scoped_tuned_and_disabled(pool: PgPool) {
    let env = Env::new(pool, &[server_json(1, "web-1", "off")]).await;
    let rules = env
        .get(&format!("/api/v1/rules?project_id={}", env.project))
        .await;
    let rule = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check_type"] == "server_status")
        .unwrap()
        .clone();
    let uri = format!("/api/v1/rules/{}", rule["id"].as_str().unwrap());
    let patch = |body: Value| call(&env.st, "PATCH", &uri, Some(&env.key), Some(body));

    for bad in [
        json!({ "params": { "for": 10 } }),
        json!({ "params": [] }),
        json!({ "selector": "env in (a" }),
        json!({ "severity": "loud" }),
        json!({ "name": "" }),
        json!({ "unknown": 1 }),
    ] {
        let (status, body) = patch(bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
    }

    let (status, body) = patch(json!({ "params": { "for_s": 0 } })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    env.tick(1).await;
    assert_eq!(env.alerts_of("server_status", "firing").await.len(), 1);

    // Out of scope (the server has env=test): the alert is closed.
    let (status, body) = patch(json!({ "selector": "env==prod" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["rule"]["selector"], "env=prod");
    env.tick(2).await;
    assert_none(&env.alerts_of("server_status", "open").await);

    let (status, body) = patch(json!({ "selector": null })).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["rule"]["selector"].is_null());
    env.tick(3).await;
    assert_eq!(env.alerts_of("server_status", "firing").await.len(), 1);

    // Disabling closes the open alert and stops evaluation.
    let (status, body) = patch(json!({ "enabled": false })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["rule"]["enabled"], false);
    assert_none(&env.alerts_of("server_status", "open").await);
    env.tick(4).await;
    assert_none(&env.alerts_of("server_status", "open").await);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn rules_and_alerts_are_tenant_scoped(pool: PgPool) {
    let env = Env::new(pool.clone(), &[server_json(1, "web-1", "off")]).await;
    for n in 1..=3 {
        env.tick(n).await;
    }
    let alert = env.alerts_of("server_status", "firing").await[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let rule = env.get("/api/v1/rules").await["rules"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let (_, other) = org_with_key(&pool, "other", Scope::Admin).await;
    for (method, uri, body) in [
        ("GET", format!("/api/v1/rules/{rule}"), None),
        (
            "PATCH",
            format!("/api/v1/rules/{rule}"),
            Some(json!({ "enabled": false })),
        ),
        ("GET", format!("/api/v1/alerts/{alert}"), None),
    ] {
        let (status, _) = call(&env.st, method, &uri, Some(&other), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    for uri in ["/api/v1/rules", "/api/v1/alerts"] {
        let (status, body) = call(&env.st, "GET", uri, Some(&other), None).await;
        assert_eq!(status, StatusCode::OK);
        let key = uri.rsplit('/').next().unwrap();
        assert!(body[key].as_array().unwrap().is_empty(), "{uri}: {body}");
    }
    // A read key may list but not change rules.
    let (_, reader) = org_with_key(&pool, "readers", Scope::Read).await;
    let (status, _) = call(
        &env.st,
        "PATCH",
        &format!("/api/v1/rules/{rule}"),
        Some(&reader),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn alert_list_validates_and_paginates(pool: PgPool) {
    let env = Env::new(
        pool,
        &[
            server_json(1, "a", "off"),
            server_json(2, "b", "off"),
            server_json(3, "c", "off"),
        ],
    )
    .await;
    env.tick(1).await;
    let (status, _) = call(
        &env.st,
        "GET",
        "/api/v1/alerts?state=bogus",
        Some(&env.key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let page = env.get("/api/v1/alerts?state=pending&per_page=2").await;
    assert_eq!(page["alerts"].as_array().unwrap().len(), 2);
    assert_eq!(page["meta"]["pagination"]["total_entries"], 3);
    assert_eq!(page["meta"]["pagination"]["next_page"], 2);
    let page2 = env
        .get("/api/v1/alerts?state=pending&per_page=2&page=2")
        .await;
    assert_eq!(page2["alerts"].as_array().unwrap().len(), 1);
    assert!(page2["meta"]["pagination"]["next_page"].is_null());
}
