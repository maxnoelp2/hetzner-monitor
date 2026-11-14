//! The OpenAPI document: valid, complete (every route of the router is in it and vice versa) and with the
//! security requirements the auth layer actually enforces.

mod common;

use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

/// Paths that need no credentials.
const PUBLIC: &[&str] = &[
    "/healthz",
    "/readyz",
    "/api/v1/openapi.json",
    "/api/v1/auth/login",
];

const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

fn document() -> Value {
    let json = hm_server::http::openapi().to_json().unwrap();
    serde_json::from_str(&json).unwrap()
}

/// `(path, method, operation)` for every operation.
fn operations(doc: &Value) -> Vec<(String, String, Value)> {
    let mut out = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            if METHODS.contains(&method.as_str()) {
                out.push((path.clone(), method.clone(), op.clone()));
            }
        }
    }
    out
}

/// Every `$ref` in `value`.
fn refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    out.push(v.as_str().unwrap().to_owned());
                } else {
                    refs(v, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

#[test]
fn document_is_valid_openapi_31() {
    let doc = document();
    assert!(
        doc["openapi"].as_str().unwrap().starts_with("3.1."),
        "{}",
        doc["openapi"]
    );
    assert_eq!(doc["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(doc["info"]["license"]["identifier"], "AGPL-3.0-only");

    let mut all = Vec::new();
    refs(&doc, &mut all);
    assert!(all.len() > 50, "too few $refs: {}", all.len());
    for r in all {
        let pointer = r.strip_prefix('#').expect("only local refs");
        assert!(doc.pointer(pointer).is_some(), "dangling $ref {r}");
    }

    let mut ids = BTreeSet::new();
    for (path, method, op) in operations(&doc) {
        let id = op["operationId"]
            .as_str()
            .unwrap_or_else(|| panic!("{method} {path}: no operationId"));
        assert!(ids.insert(id.to_owned()), "duplicate operationId {id}");
        assert!(!op["tags"].as_array().unwrap().is_empty(), "{id}: no tag");
        assert!(
            op["responses"].as_object().is_some_and(|r| !r.is_empty()),
            "{id}: no responses"
        );
    }
    assert_eq!(ids.len(), 38, "operations: {ids:?}");

    for scheme in ["api_key", "session", "csrf"] {
        assert!(
            doc["components"]["securitySchemes"][scheme].is_object(),
            "{scheme}"
        );
    }
    assert_eq!(
        doc["components"]["securitySchemes"]["session"]["name"],
        "hm_session"
    );
    assert_eq!(
        doc["components"]["securitySchemes"]["session"]["in"],
        "cookie"
    );
    assert_eq!(
        doc["components"]["securitySchemes"]["csrf"]["name"],
        "X-CSRF-Token"
    );
    let error = &doc["components"]["schemas"]["ErrorBody"];
    assert_eq!(
        error["properties"]["error"]["$ref"],
        "#/components/schemas/ErrorDetail"
    );
}

#[test]
fn security_matches_the_auth_layer() {
    let doc = document();
    for (path, method, op) in operations(&doc) {
        let security = &op["security"];
        if PUBLIC.contains(&path.as_str()) {
            assert_eq!(security, &json!([]), "{method} {path} must be public");
        } else if method == "get" {
            assert_eq!(
                security,
                &json!([{"api_key": []}, {"session": []}]),
                "{method} {path}"
            );
            assert!(op["responses"]["401"].is_object(), "{method} {path}: 401");
        } else {
            assert_eq!(
                security,
                &json!([{"api_key": []}, {"session": [], "csrf": []}]),
                "{method} {path}"
            );
            assert!(op["responses"]["403"].is_object(), "{method} {path}: 403");
        }
    }
}

#[test]
fn response_fields_are_required_and_request_options_are_not() {
    let doc = document();
    let schemas = &doc["components"]["schemas"];
    let required = |name: &str| -> Vec<String> {
        serde_json::from_value(schemas[name]["required"].clone()).unwrap_or_default()
    };
    // Option fields of responses are always serialised (as null).
    assert!(required("ProjectView").contains(&"rl_limit".to_owned()));
    assert!(required("Profile").contains(&"csrf_token".to_owned()));
    // ... except the ones skipped when empty.
    assert!(!required("TestResult").contains(&"error".to_owned()));
    // Request bodies keep their optional fields optional.
    assert!(!required("Login").contains(&"org_id".to_owned()));
    assert!(!required("CreateChannel").contains(&"min_severity".to_owned()));
}

/// Every documented operation is served by the router: never a routing 404 (empty body) or 405.
#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn every_documented_operation_reaches_a_handler(pool: PgPool) {
    let state = common::state(pool, "http://127.0.0.1:9");
    let id = uuid::Uuid::new_v4().to_string();
    for (path, method, _) in operations(&document()) {
        let uri = path.replace("{id}", &id);
        let body = (method != "get" && method != "delete").then(|| json!({}));
        let (status, _, value) =
            common::call_raw(&state, &method.to_uppercase(), &uri, &[], body).await;
        assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
        if status == StatusCode::NOT_FOUND {
            assert!(
                value["error"]["code"].is_string(),
                "{method} {path} is not routed"
            );
        }
    }
}

/// The router serves the same document the CLI prints.
#[sqlx::test(migrator = "hm_server::db::MIGRATOR")]
async fn served_document_equals_the_generated_one(pool: PgPool) {
    let state = common::state(pool, "http://127.0.0.1:9");
    let (status, served) = common::call(&state, "GET", "/api/v1/openapi.json", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(served, document());
}

/// Routes must be registered with `utoipa_axum::routes!` so they cannot be missing from the document.
#[test]
fn routes_are_only_registered_through_utoipa_axum() {
    let src = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut files = vec![format!("{src}/http.rs")];
    for entry in std::fs::read_dir(format!("{src}/api")).unwrap() {
        files.push(entry.unwrap().path().display().to_string());
    }
    for file in files {
        let code = std::fs::read_to_string(&file)
            .unwrap()
            .replace("OpenApiRouter", "");
        for forbidden in [
            ".route(",
            ".route_service(",
            ".nest_service(",
            "Router::new()",
            "axum::routing",
        ] {
            assert!(
                !code.contains(forbidden),
                "{file} uses `{forbidden}`: register routes with utoipa_axum::routes! instead"
            );
        }
    }
}
