//! Login, sessions, CSRF, password management and master-key rotation (M5a).

mod common;

use std::sync::Arc;

use axum::http::StatusCode;
use common::{
    Session, call, call_raw, call_session, login, mock_inventory, org_with_key, state,
    user_with_password,
};
use hm_core::crypto::{Keyring, MasterKey};
use hm_server::auth::Scope;
use hm_server::{bootstrap, keys};
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const PW: &str = "correct horse battery staple";
const HETZNER_OFF: &str = "http://127.0.0.1:9";

fn tomorrow() -> String {
    (time::OffsetDateTime::now_utc() + time::Duration::days(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

async fn owner(pool: &PgPool, email: &str) -> (Uuid, String) {
    let (org, key) = org_with_key(pool, "Acme", Scope::Admin).await;
    user_with_password(pool, org, email, PW, "owner").await;
    (org, key)
}

async fn attempt(
    st: &hm_server::http::AppState,
    email: &str,
    password: &str,
) -> (StatusCode, Value) {
    let (status, _, body) = call_raw(
        st,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "email": email, "password": password })),
    )
    .await;
    (status, body)
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn login_me_and_logout(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let (org, _) = owner(&pool, "Ops@Example.com").await;

    let (status, headers, body) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/login",
        &[("user-agent", "test-agent")],
        Some(json!({ "email": " ops@example.com ", "password": PW })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let cookie = headers["set-cookie"].to_str().unwrap();
    for part in [
        "hm_session=",
        "HttpOnly",
        "SameSite=Lax",
        "Secure",
        "Path=/",
        "Max-Age=2592000",
    ] {
        assert!(cookie.contains(part), "{cookie}");
    }
    assert_eq!(body["org"]["id"], org.to_string());
    assert_eq!(body["role"], "owner");
    assert_eq!(body["auth"], "session");
    assert!(!body.to_string().contains(PW));

    // Only the hash of the token is stored.
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .trim_start_matches("hm_session=");
    let (hash, agent): (Vec<u8>, Option<String>) =
        sqlx::query_as("SELECT token_hash, user_agent FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(hash, hm_core::session::hash(token));
    assert_eq!(agent.as_deref(), Some("test-agent"));

    let session = Session {
        cookie: format!("hm_session={token}"),
        csrf: body["csrf_token"].as_str().unwrap().to_owned(),
    };
    let (status, me) = call_session(&st, "GET", "/api/v1/me", &session, false, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["user"]["email"], "Ops@Example.com");
    assert_eq!(
        me["csrf_token"],
        session.csrf.as_str(),
        "stable per session"
    );
    let (status, _) = call_session(&st, "GET", "/api/v1/projects", &session, false, None).await;
    assert_eq!(status, StatusCode::OK);

    // Logout is state-changing: CSRF needed.
    let (status, body) =
        call_session(&st, "POST", "/api/v1/auth/logout", &session, false, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"]["code"], "csrf_failed");
    let (status, headers, _) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/logout",
        &[("cookie", &session.cookie), ("x-csrf-token", &session.csrf)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let (status, _) = call_session(&st, "GET", "/api/v1/me", &session, false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn plain_http_base_url_drops_secure(pool: PgPool) {
    let mut st = state(pool.clone(), HETZNER_OFF);
    st.base_url = Some(Arc::from("http://192.168.1.10:8080"));
    owner(&pool, "a@example.com").await;
    let (_, headers, _) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "email": "a@example.com", "password": PW })),
    )
    .await;
    assert!(!headers["set-cookie"].to_str().unwrap().contains("Secure"));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn bad_logins_look_the_same(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let (org, _) = owner(&pool, "a@example.com").await;
    let other = user_with_password(&pool, org, "off@example.com", PW, "member").await;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    // The key owner from org_with_key has no password at all.
    let no_pw: String =
        sqlx::query_scalar("SELECT email::text FROM users WHERE password_hash IS NULL LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

    let mut bodies = Vec::new();
    for (email, password) in [
        ("a@example.com", "wrong password!"),
        ("nobody@example.com", PW),
        ("off@example.com", PW),
        (no_pw.as_str(), PW),
        ("a@example.com", ""),
    ] {
        let (status, body) = attempt(&st, email, password).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{email}");
        bodies.push(body);
    }
    assert!(bodies.iter().all(|b| *b == bodies[0]));
    assert_eq!(bodies[0]["error"]["code"], "invalid_credentials");

    // Unknown fields and a foreign org are refused too.
    let (status, _, _) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "email": "a@example.com", "password": PW, "remember": true })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (foreign, _) = org_with_key(&pool, "Other", Scope::Read).await;
    let (status, _, _) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "email": "a@example.com", "password": PW, "org_id": foreign })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn failed_logins_are_throttled(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    owner(&pool, "a@example.com").await;
    for _ in 0..hm_server::throttle::MAX_PER_EMAIL {
        assert_eq!(
            attempt(&st, "a@example.com", "wrong password!").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, headers, body) = call_raw(
        &st,
        "POST",
        "/api/v1/auth/login",
        &[],
        Some(json!({ "email": "A@example.com", "password": PW })),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["error"]["code"], "too_many_attempts");
    let retry: u64 = headers["retry-after"].to_str().unwrap().parse().unwrap();
    assert!(retry > 0 && retry <= 900);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn csrf_is_required_for_cookie_writes_only(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let (_, key) = owner(&pool, "a@example.com").await;
    let session = login(&st, "a@example.com", PW).await;
    let silence = || Some(json!({ "ends_at": tomorrow(), "reason": "test" }));

    let (status, body) =
        call_session(&st, "POST", "/api/v1/silences", &session, false, silence()).await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::FORBIDDEN, json!("csrf_failed"))
    );
    let wrong = Session {
        csrf: hm_core::session::generate()
            .unwrap()
            .token
            .expose()
            .to_string(),
        ..session.clone()
    };
    let (status, _) = call_session(&st, "POST", "/api/v1/silences", &wrong, true, silence()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) =
        call_session(&st, "POST", "/api/v1/silences", &session, true, silence()).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    // API keys need no CSRF token.
    let (status, _) = call(&st, "POST", "/api/v1/silences", Some(&key), silence()).await;
    assert_eq!(status, StatusCode::CREATED);

    // An Authorization header is never ignored in favour of the cookie.
    let (status, _, _) = call_raw(
        &st,
        "GET",
        "/api/v1/me",
        &[
            ("cookie", &session.cookie),
            ("authorization", "Bearer hm_nope"),
        ],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // A malformed cookie is just unauthenticated.
    let (status, _, _) = call_raw(
        &st,
        "GET",
        "/api/v1/me",
        &[("cookie", "hm_session=x")],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn session_scope_follows_role(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let (org, _) = owner(&pool, "owner@example.com").await;
    user_with_password(&pool, org, "member@example.com", PW, "member").await;
    let owner = login(&st, "owner@example.com", PW).await;
    let member = login(&st, "member@example.com", PW).await;

    let (status, _) = call_session(&st, "GET", "/api/v1/api-keys", &owner, false, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call_session(&st, "GET", "/api/v1/api-keys", &member, false, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = call_session(
        &st,
        "POST",
        "/api/v1/silences",
        &member,
        true,
        Some(json!({ "ends_at": tomorrow() })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "members can write");
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn sessions_never_cross_tenants(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    owner(&pool, "a@example.com").await;
    let (_, other_key) = org_with_key(&pool, "Other", Scope::Admin).await;
    let (status, created) = call(
        &st,
        "POST",
        "/api/v1/silences",
        Some(&other_key),
        Some(json!({ "ends_at": tomorrow() })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let foreign = created["silence"]["id"].as_str().unwrap();

    let session = login(&st, "a@example.com", PW).await;
    let (_, list) = call_session(&st, "GET", "/api/v1/silences", &session, false, None).await;
    assert_eq!(list["silences"], json!([]));
    let (status, _) = call_session(
        &st,
        "DELETE",
        &format!("/api/v1/silences/{foreign}"),
        &session,
        true,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn idle_and_expired_sessions_end(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    owner(&pool, "a@example.com").await;
    let idle = login(&st, "a@example.com", PW).await;
    let old = login(&st, "a@example.com", PW).await;
    let fresh = login(&st, "a@example.com", PW).await;
    let set = |sql: &'static str, s: &Session| {
        let hash = hm_core::session::hash(s.cookie.trim_start_matches("hm_session="));
        let pool = pool.clone();
        async move { sqlx::query(sql).bind(hash).execute(&pool).await.unwrap() }
    };
    set(
        "UPDATE sessions SET last_seen_at = now() - interval '15 days' WHERE token_hash = $1",
        &idle,
    )
    .await;
    set(
        "UPDATE sessions SET expires_at = now() - interval '1 second' WHERE token_hash = $1",
        &old,
    )
    .await;
    for s in [&idle, &old] {
        let (status, _) = call_session(&st, "GET", "/api/v1/me", s, false, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = call_session(&st, "GET", "/api/v1/me", &fresh, false, None).await;
    assert_eq!(status, StatusCode::OK);

    // The next login purges them.
    login(&st, "a@example.com", PW).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);

    // Losing the membership ends the session.
    sqlx::query("DELETE FROM memberships WHERE user_id = (SELECT id FROM users WHERE email = 'a@example.com')")
        .execute(&pool)
        .await
        .unwrap();
    let (status, _) = call_session(&st, "GET", "/api/v1/me", &fresh, false, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn password_change_ends_other_sessions(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    owner(&pool, "a@example.com").await;
    let a = login(&st, "a@example.com", PW).await;
    let b = login(&st, "a@example.com", PW).await;
    let change = |current: &str, new: &str| {
        Some(json!({ "current_password": current, "new_password": new }))
    };

    let (status, body) = call_session(
        &st,
        "PUT",
        "/api/v1/me/password",
        &a,
        true,
        change("nope nope nope", "a brand new password"),
    )
    .await;
    assert_eq!(
        (status, body["error"]["code"].clone()),
        (StatusCode::FORBIDDEN, json!("wrong_password"))
    );
    let (status, _) = call_session(
        &st,
        "PUT",
        "/api/v1/me/password",
        &a,
        true,
        change(PW, "short"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call_session(
        &st,
        "PUT",
        "/api/v1/me/password",
        &a,
        false,
        change(PW, "a brand new password"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "CSRF");
    let (status, _) = call_session(
        &st,
        "PUT",
        "/api/v1/me/password",
        &a,
        true,
        change(PW, "a brand new password"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    assert_eq!(
        call_session(&st, "GET", "/api/v1/me", &a, false, None)
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        call_session(&st, "GET", "/api/v1/me", &b, false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        attempt(&st, "a@example.com", PW).await.0,
        StatusCode::UNAUTHORIZED
    );
    login(&st, "a@example.com", "a brand new password").await;
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn admin_key_sets_a_first_password(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let (org, admin) = org_with_key(&pool, "Acme", Scope::Admin).await;
    let reader = {
        let key = hm_core::apikey::generate().unwrap();
        let user: Uuid = sqlx::query_scalar("SELECT user_id FROM memberships WHERE org_id = $1")
            .bind(org)
            .fetch_one(&pool)
            .await
            .unwrap();
        hm_server::repo::api_keys::insert(
            &pool,
            hm_server::repo::OrgId(org),
            hm_server::repo::UserId(user),
            "ro",
            &[Scope::Read],
            &key,
        )
        .await
        .unwrap();
        key.full.expose().to_string()
    };
    let body = Some(json!({ "new_password": "first password here" }));
    let (status, _) = call(
        &st,
        "PUT",
        "/api/v1/me/password",
        Some(&reader),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&st, "PUT", "/api/v1/me/password", Some(&admin), body).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, me) = call(&st, "GET", "/api/v1/me", Some(&admin), None).await;
    assert_eq!(me["auth"], "api_key");
    assert_eq!(me["csrf_token"], Value::Null);
    login(
        &st,
        me["user"]["email"].as_str().unwrap(),
        "first password here",
    )
    .await;
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn bootstrap_password_and_reset(pool: PgPool) {
    let st = state(pool.clone(), HETZNER_OFF);
    let done = bootstrap::run(&pool, "Acme", "boss@example.com")
        .await
        .unwrap();
    let first = done.password.expose().to_string();
    let session = login(&st, "boss@example.com", &first).await;

    let new = bootstrap::reset_password(&pool, "boss@example.com")
        .await
        .unwrap();
    assert_ne!(new.expose().as_str(), first);
    assert_eq!(
        call_session(&st, "GET", "/api/v1/me", &session, false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        attempt(&st, "boss@example.com", &first).await.0,
        StatusCode::UNAUTHORIZED
    );
    login(&st, "boss@example.com", new.expose()).await;
    assert!(matches!(
        bootstrap::reset_password(&pool, "nobody@example.com").await,
        Err(bootstrap::BootstrapError::UnknownUser)
    ));
}

#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn key_rotation_end_to_end(pool: PgPool) {
    let hetzner = MockServer::start().await;
    mock_inventory(&hetzner, &[(1, "web-1")]).await;
    let hooks = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&hooks)
        .await;
    let one = common::TEST_MASTER_KEY;
    let two = MasterKey::generate_base64().unwrap();
    let ring = |list: &str| Arc::new(Keyring::parse(list).unwrap());

    // Secrets sealed with version 1.
    let mut st = state(pool.clone(), &hetzner.uri());
    let (_, key) = org_with_key(&pool, "Acme", Scope::Admin).await;
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
    let (status, body) = call(
        &st,
        "POST",
        "/api/v1/channels",
        Some(&key),
        Some(json!({ "type": "webhook", "name": "hook",
                     "config": { "url": format!("{}/hook", hooks.uri()), "secret": "0123456789abcdef" } })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let channel = body["channel"]["id"].as_str().unwrap().to_owned();

    // New keyring: v1 still opens, new secrets use v2, startup check passes.
    let both = ring(&format!("1:{one},2:{}", two.as_str()));
    keys::check_versions(&pool, &both).await.unwrap();
    let only_two = ring(&format!("2:{}", two.as_str()));
    assert!(matches!(
        keys::check_versions(&pool, &only_two).await,
        Err(keys::KeysError::MissingVersions(v)) if v == [1]
    ));

    let done = keys::rotate(&pool, &both).await.unwrap();
    assert_eq!(
        done,
        keys::Rotated {
            projects: 1,
            channels: 1,
            ..keys::Rotated::default()
        }
    );
    let status = keys::status(&pool).await.unwrap();
    assert_eq!(status.versions().collect::<Vec<_>>(), [2]);
    assert_eq!(
        keys::rotate(&pool, &both).await.unwrap(),
        keys::Rotated::default()
    );

    // The old key can go: everything still decrypts with v2 alone.
    keys::check_versions(&pool, &only_two).await.unwrap();
    st.keyring = only_two;
    let (status, body) = call(
        &st,
        "POST",
        &format!("/api/v1/projects/{project}/sync"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &st,
        "POST",
        &format!("/api/v1/channels/{channel}/test"),
        Some(&key),
        None,
    )
    .await;
    assert_eq!(
        (status, body["ok"].clone()),
        (StatusCode::OK, json!(true)),
        "{body}"
    );

    // A keyring without the stored version cannot rotate.
    assert!(
        keys::rotate(&pool, &ring(&format!("3:{one}")))
            .await
            .is_err()
    );
}
