//! M6b: Web Push — VAPID key, push-subscription API, `web_push` channels fanning out per device,
//! `410 Gone` handling and key rotation. Push services are wiremock servers; the RFC 8291
//! encryption itself is unit-tested against the RFC test vector in `hm-notify`.

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{call, mock_project, org_with_key, server_json, state};
use hm_core::crypto::{self, Keyring, MasterKey};
use hm_server::auth::Scope;
use hm_server::http::AppState;
use hm_server::push::PushState;
use hm_server::repo::push::NewSubscription;
use hm_server::repo::{OrgId, UserId};
use hm_server::{keys, notifier, scheduler};
use serde_json::{Value, json};
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A valid browser key pair's public half and auth secret (RFC 8291, Appendix A).
const P256DH: &str =
    "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";

fn subscription(endpoint: &str, name: &str) -> Value {
    json!({ "endpoint": endpoint, "keys": { "p256dh": P256DH, "auth": AUTH },
            "expirationTime": null, "name": name })
}

async fn user_of_key(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar("SELECT user_id FROM memberships WHERE org_id = $1 AND role = 'owner'")
        .bind(org)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Registers a device directly (the API only accepts `https://` endpoints; wiremock is http).
async fn add_device(st: &AppState, org: Uuid, user: Uuid, endpoint: &str, name: &str) -> Uuid {
    let sub = hm_notify::push::Subscription {
        endpoint: endpoint.to_owned(),
        p256dh: P256DH.to_owned(),
        auth: AUTH.to_owned(),
    };
    let id = Uuid::new_v4();
    let sealed = crypto::seal(
        &st.keyring,
        id.as_bytes(),
        &serde_json::to_vec(&sub).unwrap(),
    )
    .unwrap();
    let mut conn = st.db.acquire().await.unwrap();
    hm_server::repo::push::insert(
        &mut conn,
        OrgId(org),
        UserId(user),
        &NewSubscription {
            id,
            endpoint_hash: &sub.endpoint_hash(),
            sealed: &sealed,
            name,
            host: &sub.host(),
        },
        OffsetDateTime::now_utc(),
    )
    .await
    .unwrap()
    .expect("inserted");
    id
}

async fn push_service(status: u16) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("content-encoding", "aes128gcm"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    server
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn push_config_and_subscription_api(pool: PgPool) {
    let hetzner = MockServer::start().await;
    let st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "acme", Scope::Write).await;
    let (_, other) = org_with_key(&pool, "other", Scope::Admin).await;

    let (status, body) = call(&st, "GET", "/api/v1/push/config", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], true);
    let public = body["vapid_public_key"].as_str().unwrap().to_owned();
    assert_eq!(public.len(), 87, "65-byte point, base64url");
    // One key pair per installation, the same for every org and on every call.
    let (_, body) = call(&st, "GET", "/api/v1/push/config", Some(&other), None).await;
    assert_eq!(body["vapid_public_key"], public.as_str());
    let fresh = AppState {
        push: PushState::new(Some("mailto:ops@hm.test")),
        ..st.clone()
    };
    let (_, body) = call(&fresh, "GET", "/api/v1/push/config", Some(&key), None).await;
    assert_eq!(
        body["vapid_public_key"],
        public.as_str(),
        "loaded from the database"
    );

    let endpoint = "https://push.example.net/send/sekrit-endpoint";
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription(endpoint, "Pixel · Chrome")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["subscription"]["id"].as_str().unwrap().to_owned();
    assert_eq!(body["subscription"]["host"], "push.example.net");
    assert_eq!(body["subscription"]["name"], "Pixel · Chrome");
    assert!(!body.to_string().contains("sekrit") && !body.to_string().contains(AUTH));

    // Same endpoint again: same row, new name, 200.
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription(endpoint, "Pixel")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["subscription"]["id"], id.as_str());
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&key), None).await;
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);
    assert_eq!(body["subscriptions"][0]["name"], "Pixel");
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT subscription_ct FROM push_subscriptions WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        !String::from_utf8_lossy(&stored).contains("sekrit"),
        "sealed at rest"
    );

    // Invalid subscriptions are rejected without echoing them.
    for bad in [
        subscription("http://push.example.net/sekrit", "x"),
        subscription("not a url sekrit", "x"),
        json!({ "endpoint": endpoint, "keys": { "p256dh": "sekrit", "auth": AUTH } }),
        json!({ "endpoint": endpoint, "keys": { "p256dh": P256DH, "auth": "c2Vrcml0" } }),
        json!({ "endpoint": endpoint }),
    ] {
        let (status, body) = call(
            &st,
            "POST",
            "/api/v1/push/subscriptions",
            Some(&key),
            Some(bad),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(!body.to_string().contains("sekrit"), "{body}");
    }

    // Other tenants neither see nor remove it.
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&other), None).await;
    assert_eq!(body["subscriptions"], json!([]));
    let uri = format!("/api/v1/push/subscriptions/{id}");
    let (status, _) = call(&st, "DELETE", &uri, Some(&other), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&st, "POST", &format!("{uri}/test"), Some(&other), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = call(&st, "DELETE", &uri, Some(&key), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&st, "DELETE", &uri, Some(&key), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&key), None).await;
    assert_eq!(body["subscriptions"], json!([]));
    // The tombstone keeps no secret; registering the endpoint again creates a new row.
    let wiped: Vec<u8> =
        sqlx::query_scalar("SELECT subscription_ct FROM push_subscriptions WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wiped, Vec::<u8>::new());
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription(endpoint, "Pixel")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_ne!(body["subscription"]["id"], id.as_str());

    // Without a VAPID subject push is off: config says so, subscribing and web_push channels fail.
    let off = AppState {
        push: PushState::new(None),
        ..st.clone()
    };
    let (_, body) = call(&off, "GET", "/api/v1/push/config", Some(&key), None).await;
    assert_eq!(body, json!({ "enabled": false }));
    let (status, body) = call(
        &off,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription(endpoint, "x")),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("push_disabled"))
    );
    let (status, body) = call(
        &off,
        "POST",
        "/api/v1/channels",
        Some(&key),
        Some(json!({ "type": "web_push", "name": "phones", "config": {} })),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("push_disabled"))
    );
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn web_push_channel_targets_members_and_tests_each_device(pool: PgPool) {
    let hetzner = MockServer::start().await;
    let st = state(pool.clone(), &hetzner.uri());
    let (org, key) = org_with_key(&pool, "acme", Scope::Admin).await;
    let owner = user_of_key(&pool, org).await;
    let colleague = common::user_with_password(
        &pool,
        org,
        "colleague@example.com",
        "a long test password",
        "member",
    )
    .await;
    let (other_org, _) = org_with_key(&pool, "other", Scope::Admin).await;
    let stranger = user_of_key(&pool, other_org).await;

    let ok = push_service(201).await;
    let gone = push_service(410).await;
    let phone = add_device(&st, org, owner, &format!("{}/phone", ok.uri()), "Phone").await;
    let old = add_device(
        &st,
        org,
        owner,
        &format!("{}/old", gone.uri()),
        "Old tablet",
    )
    .await;
    let theirs = add_device(&st, org, colleague, &format!("{}/c", ok.uri()), "Laptop").await;
    // A device in another org is never targeted.
    add_device(&st, other_org, stranger, &format!("{}/x", ok.uri()), "X").await;

    // `users` must be members of the org.
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/channels",
        Some(&key),
        Some(json!({ "type": "web_push", "name": "p", "config": { "users": [stranger] } })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/channels",
        Some(&key),
        Some(json!({ "type": "web_push", "name": "mine", "config": { "users": [owner] } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["channel"]["hint"], "1 user");
    let mine = body["channel"]["id"].as_str().unwrap().to_owned();

    let (status, body) = call(
        &st,
        "POST",
        &format!("/api/v1/channels/{mine}/test"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false, "{body}");
    assert_eq!(body["error"], "1 of 2 devices failed");
    let devices = body["devices"].as_array().unwrap();
    assert_eq!(devices.len(), 2, "only the owner's devices: {body}");
    assert_eq!(devices[0]["id"], phone.to_string());
    assert_eq!(devices[0]["ok"], true);
    assert_eq!(devices[1]["id"], old.to_string());
    assert_eq!(devices[1]["error"], "device unsubscribed (HTTP 410)");

    // The gone device was removed; the next test reaches only the phone.
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&key), None).await;
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);
    let (_, body) = call(
        &st,
        "POST",
        &format!("/api/v1/channels/{mine}/test"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(body["ok"], true, "{body}");

    // Switching to every member reaches the colleague's laptop too.
    let (status, body) = call(
        &st,
        "PATCH",
        &format!("/api/v1/channels/{mine}"),
        Some(&key),
        Some(json!({ "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["channel"]["hint"], "all members");
    let (_, body) = call(
        &st,
        "POST",
        &format!("/api/v1/channels/{mine}/test"),
        Some(&key),
        None,
    )
    .await;
    let ids: Vec<&str> = body["devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [phone.to_string(), theirs.to_string()]);

    // A test push to one own device.
    let (status, body) = call(
        &st,
        "POST",
        &format!("/api/v1/push/subscriptions/{phone}/test"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "ok": true }));
    // The colleague's device is not the caller's.
    let (status, _) = call(
        &st,
        "POST",
        &format!("/api/v1/push/subscriptions/{theirs}/test"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

const STEP: i64 = 66;

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn alerts_fan_out_per_device_and_skip_gone_devices(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_project(&hetzner, &[server_json(1, "web-1", "running")], &[]).await;
    let st = state(pool.clone(), &hetzner.uri());
    let (org, key) = org_with_key(&pool, "acme", Scope::Admin).await;
    let owner = user_of_key(&pool, org).await;
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/projects",
        Some(&key),
        Some(json!({ "name": "prod", "token": common::TOKEN })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let devices = MockServer::start().await;
    for (route, status) in [("/phone", 201), ("/old", 410)] {
        Mock::given(method("POST"))
            .and(path(route))
            .respond_with(ResponseTemplate::new(status))
            .mount(&devices)
            .await;
    }
    let phone = add_device(
        &st,
        org,
        owner,
        &format!("{}/phone", devices.uri()),
        "Phone",
    )
    .await;
    add_device(
        &st,
        org,
        owner,
        &format!("{}/old", devices.uri()),
        "Old tablet",
    )
    .await;
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/channels",
        Some(&key),
        Some(json!({ "type": "web_push", "name": "phones", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    // A web_push channel nobody subscribed to gets no jobs.
    let (_, key2) = org_with_key(&pool, "empty", Scope::Admin).await;
    let (status, _) = call(
        &st,
        "POST",
        "/api/v1/channels",
        Some(&key2),
        Some(json!({ "type": "web_push", "name": "phones", "config": {} })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let t0 = OffsetDateTime::now_utc();
    let at = |s: i64| t0 + Duration::seconds(s);
    mock_project(&hetzner, &[server_json(1, "web-1", "off")], &[]).await;
    scheduler::tick(&st, at(STEP)).await.unwrap();
    scheduler::tick(&st, at(3 * STEP)).await.unwrap();
    let (_, body) = call(&st, "GET", "/api/v1/alerts?state=open", Some(&key), None).await;
    let alert = body["alerts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["check_type"] == "server_status")
        .expect("server_status alert")["id"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(notifier::run_once(&st, at(3 * STEP)).await.unwrap(), 2);
    let (_, detail) = call(
        &st,
        "GET",
        &format!("/api/v1/alerts/{alert}"),
        Some(&key),
        None,
    )
    .await;
    let jobs = detail["notifications"].as_array().expect("notifications");
    assert_eq!(jobs.len(), 2, "{detail}");
    let by_device = |name: &str| {
        jobs.iter()
            .find(|j| j["device"] == name)
            .unwrap_or_else(|| panic!("job for {name}: {detail}"))
            .clone()
    };
    assert_eq!(by_device("Phone")["status"], "sent");
    let old_job = by_device("Old tablet");
    assert_eq!(old_job["status"], "failed");
    assert_eq!(old_job["last_error"], "device unsubscribed (HTTP 410)");
    assert_eq!(old_job["attempts"], 1, "gone is not retried");

    // The gone device was removed; the resolve goes to the phone only.
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&key), None).await;
    assert_eq!(body["subscriptions"][0]["id"], phone.to_string());
    assert_eq!(body["subscriptions"].as_array().unwrap().len(), 1);
    mock_project(&hetzner, &[server_json(1, "web-1", "running")], &[]).await;
    scheduler::tick(&st, at(4 * STEP)).await.unwrap();
    scheduler::tick(&st, at(5 * STEP)).await.unwrap();
    assert_eq!(notifier::run_once(&st, at(5 * STEP)).await.unwrap(), 1);
    let received = devices.received_requests().await.unwrap();
    let paths: Vec<&str> = received.iter().map(|r| r.url.path()).collect();
    assert_eq!(paths, ["/phone", "/old", "/phone"]);
    for r in &received {
        assert_eq!(r.headers["content-encoding"], "aes128gcm");
        assert!(
            r.headers["authorization"]
                .to_str()
                .unwrap()
                .starts_with("vapid t=")
        );
        assert_eq!(r.headers["ttl"], "86400", "critical alert");
    }
    let (_, detail) = call(
        &st,
        "GET",
        &format!("/api/v1/alerts/{alert}"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(detail["notifications"].as_array().unwrap().len(), 3);
    // The removed device's job history stays visible under its name.
    assert!(detail.to_string().contains("Old tablet"));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn key_rotation_covers_push_secrets(pool: PgPool) {
    let hetzner = MockServer::start().await;
    let mut st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "acme", Scope::Admin).await;
    let (status, _) = call(&st, "GET", "/api/v1/push/config", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription("https://push.example.net/a", "Phone")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/push/subscriptions",
        Some(&key),
        Some(subscription("https://push.example.net/b", "Removed")),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let removed = body["subscription"]["id"].as_str().unwrap().to_owned();
    let (status, _) = call(
        &st,
        "DELETE",
        &format!("/api/v1/push/subscriptions/{removed}"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let status = keys::status(&pool).await.unwrap();
    assert_eq!(
        status.rows.get(&("push_subscriptions", 1)),
        Some(&1),
        "tombstones hold no secret"
    );
    assert_eq!(status.rows.get(&("vapid_keys", 1)), Some(&1));

    let two = MasterKey::generate_base64().unwrap();
    let both = Arc::new(
        Keyring::parse(&format!("1:{},2:{}", common::TEST_MASTER_KEY, two.as_str())).unwrap(),
    );
    let done = keys::rotate(&pool, &both).await.unwrap();
    assert_eq!((done.push_subscriptions, done.vapid_keys), (1, 1));
    assert_eq!(
        keys::status(&pool)
            .await
            .unwrap()
            .versions()
            .collect::<Vec<_>>(),
        [2]
    );

    // Only v2 left: the VAPID key still opens (fresh cache) and the device still decrypts.
    let only_two = Arc::new(Keyring::parse(&format!("2:{}", two.as_str())).unwrap());
    keys::check_versions(&pool, &only_two).await.unwrap();
    st.keyring = only_two;
    st.push = PushState::new(Some("mailto:ops@hm.test"));
    let (_, body) = call(&st, "GET", "/api/v1/push/subscriptions", Some(&key), None).await;
    let phone: Uuid = body["subscriptions"][0]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let (secret_ct, dek_ct, key_version): (Vec<u8>, Vec<u8>, i32) = sqlx::query_as(
        "SELECT subscription_ct, subscription_dek_ct, key_version FROM push_subscriptions WHERE id = $1",
    )
    .bind(phone)
    .fetch_one(&pool)
    .await
    .unwrap();
    let sub = notifier::decrypt_subscription(
        &st,
        phone,
        &crypto::Sealed {
            secret_ct,
            dek_ct,
            key_version,
        },
    )
    .unwrap();
    assert_eq!(sub.endpoint, "https://push.example.net/a");
    let (status, body) = call(&st, "GET", "/api/v1/push/config", Some(&key), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
