//! M4: channels, the notification queue, the notifier, silences and ack against a mocked
//! Hetzner API and mocked channel endpoints. Cycles and deliveries run on a synthetic clock.

mod common;

use axum::http::StatusCode;
use common::{call, mock_project, org_with_key, server_json, state_with_mail};
use hm_server::auth::Scope;
use hm_server::http::AppState;
use hm_server::{notifier, scheduler};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STEP: i64 = 66;
const SECRET: &str = "0123456789abcdef-test";

struct Env {
    st: AppState,
    mail: hm_notify::StubMail,
    hetzner: MockServer,
    hooks: MockServer,
    key: String,
    project: String,
    t0: OffsetDateTime,
}

impl Env {
    async fn new(pool: PgPool) -> Self {
        let hetzner = MockServer::start().await;
        mock_project(&hetzner, &[server_json(1, "web-1", "running")], &[]).await;
        let hooks = MockServer::start().await;
        let (st, mail) = state_with_mail(pool.clone(), &hetzner.uri());
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
        Self {
            st,
            mail,
            hetzner,
            hooks,
            key,
            project: body["project"]["id"].as_str().unwrap().to_owned(),
            t0: OffsetDateTime::now_utc(),
        }
    }

    fn at(&self, s: i64) -> OffsetDateTime {
        self.t0 + Duration::seconds(s)
    }

    async fn tick(&self, n: i64) {
        scheduler::tick(&self.st, self.at(n * STEP)).await.unwrap();
    }

    async fn deliver(&self, s: i64) -> usize {
        notifier::run_once(&self.st, self.at(s)).await.unwrap()
    }

    async fn req(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        call(&self.st, method, uri, Some(&self.key), body).await
    }

    async fn channel(&self, kind: &str, config: Value, extra: Value) -> String {
        let mut body =
            json!({ "type": kind, "name": format!("{kind}-{}", rand_suffix()), "config": config });
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        let (status, body) = self.req("POST", "/api/v1/channels", Some(body)).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["channel"]["id"].as_str().unwrap().to_owned()
    }

    async fn webhook(&self, route: &str) -> String {
        self.channel(
            "webhook",
            json!({ "url": format!("{}{route}", self.hooks.uri()), "secret": SECRET }),
            json!({}),
        )
        .await
    }

    /// Powers the server off and ticks until the `server_status` alert fires. Returns its id.
    async fn fire(&self) -> String {
        mock_project(&self.hetzner, &[server_json(1, "web-1", "off")], &[]).await;
        self.tick(1).await;
        self.tick(3).await; // 198 s ≥ 120 s hold
        let alert = self.open_alert().await;
        assert_eq!(alert["state"], "firing", "{alert}");
        alert["id"].as_str().unwrap().to_owned()
    }

    async fn power_on(&self, from: i64) {
        mock_project(&self.hetzner, &[server_json(1, "web-1", "running")], &[]).await;
        self.tick(from).await;
        self.tick(from + 1).await; // flapping guard: two good cycles
    }

    async fn open_alert(&self) -> Value {
        let (_, body) = self
            .req(
                "GET",
                &format!("/api/v1/alerts?project_id={}&state=open", self.project),
                None,
            )
            .await;
        body["alerts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["check_type"] == "server_status")
            .cloned()
            .expect("an open server_status alert")
    }

    async fn detail(&self, id: &str) -> Value {
        let (status, body) = self.req("GET", &format!("/api/v1/alerts/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    async fn hook_bodies(&self) -> Vec<Value> {
        self.hooks
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::from_slice(&r.body).unwrap())
            .collect()
    }
}

fn rand_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()
}

async fn ok_hook(server: &MockServer, route: &str) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
}

fn types(detail: &Value, key: &str, field: &str) -> Vec<String> {
    detail[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e[field].as_str().unwrap().to_owned())
        .collect()
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn channel_configs_are_write_only_and_tenant_scoped(pool: PgPool) {
    let env = Env::new(pool.clone()).await;
    let id = env.webhook("/hook").await;
    let (_, body) = env
        .req("GET", &format!("/api/v1/channels/{id}"), None)
        .await;
    let text = body.to_string();
    assert_eq!(body["channel"]["hint"], "127.0.0.1");
    assert_eq!(body["channel"]["min_severity"], "warning");
    assert!(!text.contains(SECRET) && !text.contains("/hook"), "{text}");

    // Stored sealed: neither the secret nor the URL appears in the row.
    let raw: Vec<u8> = sqlx::query_scalar("SELECT config_ct FROM notification_channels")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&raw).contains(SECRET));

    // Duplicate name → 409; invalid config → 400 without echoing the value.
    let name = body["channel"]["name"].clone();
    let (status, _) = env
        .req(
            "POST",
            "/api/v1/channels",
            Some(json!({ "type": "slack", "name": name, "config": { "url": "https://hooks.slack.test/x" } })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, err) = env
        .req(
            "POST",
            "/api/v1/channels",
            Some(json!({ "type": "webhook", "name": "w", "config": { "url": "https://x.test", "secret": "tiny-sekrit" } })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!err.to_string().contains("tiny-sekrit"));

    // Another org sees nothing and can change nothing.
    let (_, other) = org_with_key(&pool, "other", Scope::Admin).await;
    for (m, uri, body) in [
        ("GET", format!("/api/v1/channels/{id}"), None),
        (
            "PATCH",
            format!("/api/v1/channels/{id}"),
            Some(json!({ "enabled": false })),
        ),
        ("POST", format!("/api/v1/channels/{id}/test"), None),
        ("DELETE", format!("/api/v1/channels/{id}"), None),
    ] {
        let (status, _) = call(&env.st, m, &uri, Some(&other), body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{m} {uri}");
    }
    let (_, list) = call(&env.st, "GET", "/api/v1/channels", Some(&other), None).await;
    assert_eq!(list["channels"], json!([]));

    // Without SMTP settings email channels are refused.
    let mut no_mail = env.st.clone();
    no_mail.notifier = hm_notify::Notifier::new(None, true).unwrap();
    let (status, body) = call(
        &no_mail,
        "POST",
        "/api/v1/channels",
        Some(&env.key),
        Some(json!({ "type": "email", "name": "mail", "config": { "to": ["ops@example.com"] } })),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn channel_test_reports_success_and_redacted_failure(pool: PgPool) {
    let env = Env::new(pool).await;
    ok_hook(&env.hooks, "/ok").await;
    Mock::given(method("POST"))
        .and(path("/gone"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&env.hooks)
        .await;
    let ok = env.webhook("/ok").await;
    let gone = env.webhook("/gone").await;
    let (status, body) = env
        .req("POST", &format!("/api/v1/channels/{ok}/test"), None)
        .await;
    assert_eq!((status, body), (StatusCode::OK, json!({ "ok": true })));
    let sent = &env.hooks.received_requests().await.unwrap()[0];
    let payload: Value = serde_json::from_slice(&sent.body).unwrap();
    assert_eq!(payload["event"], "test");
    assert_eq!(payload["version"], 1);
    let (_, body) = env
        .req("POST", &format!("/api/v1/channels/{gone}/test"), None)
        .await;
    assert_eq!(
        body,
        json!({ "ok": false, "error": "HTTP 404", "retryable": false })
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn firing_and_resolved_are_delivered_signed(pool: PgPool) {
    let env = Env::new(pool).await;
    ok_hook(&env.hooks, "/hook").await;
    env.webhook("/hook").await;
    let id = env.fire().await;
    assert_eq!(env.deliver(200).await, 1);
    assert_eq!(env.deliver(201).await, 0, "nothing left");

    let req = &env.hooks.received_requests().await.unwrap()[0];
    let expected = hm_notify::render::signature(SECRET.as_bytes(), &req.body);
    assert_eq!(
        req.headers.get("X-HM-Signature").unwrap().to_str().unwrap(),
        expected
    );
    assert_eq!(req.headers.get("X-HM-Event").unwrap(), "fired");
    let fired: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(fired["alert"]["id"], id.as_str());
    assert_eq!(fired["alert"]["severity"], "critical");
    assert_eq!(fired["alert"]["message"], "server web-1 (1) is off");
    assert_eq!(fired["resource"]["name"], "web-1");
    assert_eq!(fired["link"], format!("https://hm.test/alerts/{id}"));

    env.power_on(4).await;
    assert_eq!(env.deliver(400).await, 1);
    let bodies = env.hook_bodies().await;
    assert_eq!(bodies[1]["event"], "resolved");
    assert_eq!(bodies[1]["alert"]["state"], "resolved");
    let detail = env.detail(&id).await;
    assert_eq!(types(&detail, "notifications", "status"), ["sent", "sent"]);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn failed_deliveries_back_off_and_honour_retry_after(pool: PgPool) {
    let env = Env::new(pool).await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .mount(&env.hooks)
        .await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
        .up_to_n_times(1)
        .mount(&env.hooks)
        .await;
    ok_hook(&env.hooks, "/hook").await;
    env.webhook("/hook").await;
    let id = env.fire().await;

    assert_eq!(env.deliver(200).await, 1); // 500 → retry in 30 s
    assert_eq!(env.deliver(229).await, 0);
    assert_eq!(env.deliver(230).await, 1); // 429 → retry in 120 s
    assert_eq!(env.deliver(349).await, 0);
    assert_eq!(env.deliver(350).await, 1); // 204
    let jobs = &env.detail(&id).await["notifications"];
    assert_eq!(jobs[0]["status"], "sent");
    assert_eq!(jobs[0]["attempts"], 3);
    assert_eq!(jobs[0]["last_error"], Value::Null);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn permanent_errors_fail_the_job_without_leaking_the_target(pool: PgPool) {
    let env = Env::new(pool).await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(410))
        .mount(&env.hooks)
        .await;
    env.webhook("/hook").await;
    let id = env.fire().await;
    assert_eq!(env.deliver(200).await, 1);
    let jobs = &env.detail(&id).await["notifications"];
    assert_eq!(jobs[0]["status"], "failed");
    assert_eq!(jobs[0]["last_error"], "HTTP 410");
    assert_eq!(env.deliver(10_000).await, 0);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn a_silence_suppresses_and_records(pool: PgPool) {
    let env = Env::new(pool.clone()).await;
    ok_hook(&env.hooks, "/hook").await;
    env.webhook("/hook").await;
    let ends = env
        .at(86_400)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let (status, body) = env
        .req(
            "POST",
            "/api/v1/silences",
            Some(json!({ "project_id": env.project, "selector": "env=test", "ends_at": ends, "reason": "maintenance" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = env.fire().await;
    assert_eq!(env.deliver(200).await, 0);
    let detail = env.detail(&id).await;
    assert_eq!(
        types(&detail, "events", "type"),
        ["pending", "fired", "suppressed"]
    );
    assert_eq!(detail["notifications"], json!([]));

    // Other orgs neither see nor end it, nor can they silence this org's project.
    let silence = body["silence"]["id"].as_str().unwrap();
    let (_, other) = org_with_key(&pool, "other", Scope::Admin).await;
    let (_, list) = call(&env.st, "GET", "/api/v1/silences", Some(&other), None).await;
    assert_eq!(list["silences"], json!([]));
    let (status, _) = call(
        &env.st,
        "DELETE",
        &format!("/api/v1/silences/{silence}"),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &env.st,
        "POST",
        "/api/v1/silences",
        Some(&other),
        Some(json!({ "project_id": env.project, "ends_at": ends })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Ending the silence does not recall anything.
    let (status, _) = env
        .req("DELETE", &format!("/api/v1/silences/{silence}"), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, active) = env.req("GET", "/api/v1/silences", None).await;
    assert_eq!(active["silences"], json!([]));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn renotify_repeats_until_acknowledged(pool: PgPool) {
    let env = Env::new(pool).await;
    ok_hook(&env.hooks, "/hook").await;
    env.webhook("/hook").await;
    let id = env.fire().await;
    env.deliver(200).await;

    // 4 h later the alert is still firing: one re-notify.
    env.tick(222).await; // 14 652 s after t0, ≥ 4 h after firing at 198 s
    env.deliver(14_700).await;
    let bodies = env.hook_bodies().await;
    assert_eq!(bodies.len(), 2);
    assert_eq!(bodies[1]["event"], "renotified");

    let (status, body) = env
        .req("POST", &format!("/api/v1/alerts/{id}/ack"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["alert"]["acked_at"].is_string());
    let (status, _) = env
        .req("POST", &format!("/api/v1/alerts/{id}/ack"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "idempotent");

    env.tick(450).await; // another 4 h+: acknowledged, so no re-notify
    env.deliver(30_000).await;
    assert_eq!(env.hook_bodies().await.len(), 2);

    // `resolved` is still sent; acking a resolved alert is a conflict.
    env.power_on(451).await;
    env.deliver(30_200).await;
    assert_eq!(env.hook_bodies().await[2]["event"], "resolved");
    let (status, _) = env
        .req("POST", &format!("/api/v1/alerts/{id}/ack"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let detail = env.detail(&id).await;
    assert_eq!(
        types(&detail, "events", "type"),
        ["pending", "fired", "renotified", "acked", "resolved"]
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn routing_by_severity_and_pinned_channels(pool: PgPool) {
    let env = Env::new(pool).await;
    for r in ["/warn", "/crit", "/pin"] {
        ok_hook(&env.hooks, r).await;
    }
    let url = |r: &str| json!({ "url": format!("{}{r}", env.hooks.uri()), "secret": SECRET });
    env.channel(
        "webhook",
        url("/warn"),
        json!({ "min_severity": "warning" }),
    )
    .await;
    env.channel(
        "webhook",
        url("/crit"),
        json!({ "min_severity": "critical", "enabled": false }),
    )
    .await;
    let pin = env
        .channel(
            "webhook",
            url("/pin"),
            json!({ "min_severity": "critical" }),
        )
        .await;

    // Unpinned: severity routing (critical alert → /warn; /crit is disabled; /pin reaches critical too).
    env.fire().await;
    env.deliver(200).await;
    let mut paths: Vec<String> = env
        .hooks
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    paths.sort();
    assert_eq!(paths, ["/pin", "/warn"]);

    // Pin the server_status rule to /pin only: the next alert goes there alone.
    env.power_on(4).await;
    env.deliver(400).await; // resolved to both channels the fired went to
    env.hooks.reset().await;
    ok_hook(&env.hooks, "/warn").await;
    ok_hook(&env.hooks, "/pin").await;
    let (_, rules) = env
        .req(
            "GET",
            &format!("/api/v1/rules?project_id={}", env.project),
            None,
        )
        .await;
    let rule = rules["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["check_type"] == "server_status")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (status, body) = env
        .req(
            "PATCH",
            &format!("/api/v1/rules/{rule}"),
            Some(json!({ "channel_ids": [pin] })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["channel_ids"], json!([pin]));
    mock_project(&env.hetzner, &[server_json(1, "web-1", "off")], &[]).await;
    env.tick(6).await;
    env.tick(8).await;
    env.deliver(600).await;
    let paths: Vec<String> = env
        .hooks
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect();
    assert_eq!(paths, ["/pin"]);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn jobs_survive_a_restart_and_locked_jobs_are_skipped(pool: PgPool) {
    let env = Env::new(pool.clone()).await;
    ok_hook(&env.hooks, "/hook").await;
    env.webhook("/hook").await;
    env.fire().await;

    // Another worker holds the job: this one skips it.
    let mut other = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM notification_jobs FOR UPDATE")
        .execute(&mut *other)
        .await
        .unwrap();
    assert_eq!(env.deliver(200).await, 0);
    other.rollback().await.unwrap();

    // A fresh process (new state, same database) sends the queued job.
    let (fresh, _) = state_with_mail(pool, &env.hetzner.uri());
    assert_eq!(notifier::run_once(&fresh, env.at(201)).await.unwrap(), 1);
    assert_eq!(env.hook_bodies().await.len(), 1);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn email_channels_send_through_smtp(pool: PgPool) {
    let env = Env::new(pool).await;
    env.channel("email", json!({ "to": ["ops@example.com"] }), json!({}))
        .await;
    env.fire().await;
    assert_eq!(env.deliver(200).await, 1);
    let sent = env.mail.messages().await;
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].1.contains("Subject: FIRING [critical]"),
        "{}",
        sent[0].1
    );
    assert!(sent[0].1.contains("server web-1 (1) is off"));
}
