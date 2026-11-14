//! OpenAPI 3.1 document, generated from the handlers' `#[utoipa::path]` attributes (docs/design/api.md).
//!
//! Routes are registered only through `utoipa_axum` (see [`crate::http`]), so every route of the router is in the
//! document. This module adds what applies to all operations: metadata, security schemes and the security
//! requirement per operation (derived from the HTTP method exactly like [`crate::auth`] decides), and the shared
//! `401`/`403` responses.

use serde::Serialize;
use utoipa::ToSchema;
use utoipa::openapi::path::Operation;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityRequirement, SecurityScheme};
use utoipa::openapi::{OpenApi, RefOr, Response, Schema};

/// Operations that need no credentials.
const PUBLIC: &[&str] = &[
    "/healthz",
    "/readyz",
    "/api/v1/openapi.json",
    "/api/v1/auth/login",
];

/// Request bodies: their `Option` fields may be omitted. In every other schema (responses), all fields are
/// serialised, `None` as `null`, so [`finish`] marks them required.
const REQUEST_SCHEMAS: &[&str] = &[
    "Login",
    "ChangePassword",
    "CreateProject",
    "CreateRule",
    "UpdateRule",
    "CreateChannel",
    "UpdateChannel",
    "ChannelConfig",
    "EmailConfig",
    "UrlConfig",
    "WebhookConfig",
    "NtfyConfig",
    "CreateSilence",
    "CreateKey",
];

/// Response fields left out when `None` (`#[serde(skip_serializing_if)]`), as `schema.field`.
const SPARSE_FIELDS: &[&str] = &["Health.reason", "TestResult.error", "TestResult.retryable"];

/// Top-level document; paths are added by the router.
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "hetzner-monitor",
        description = "Monitoring and alerting for Hetzner Cloud. Authenticate with an API key \
                       (`Authorization: Bearer hm_…`) or the `hm_session` cookie from `POST /api/v1/auth/login`; \
                       cookie requests other than GET/HEAD/OPTIONS also need `X-CSRF-Token`. \
                       Errors: `{\"error\": {\"code\", \"message\"}}`.",
        license(name = "AGPL-3.0-only", identifier = "AGPL-3.0-only"),
    ),
    servers((url = "/", description = "This server")),
    // Schemas only referenced from query parameters are not collected automatically.
    components(schemas(ErrorBody, ErrorCode, enums::AlertFilter, enums::ResourceKind)),
    tags(
        (name = "auth", description = "Login, logout and the current user"),
        (name = "projects", description = "Connected Hetzner Cloud projects and their inventory"),
        (name = "rules", description = "Alert rules"),
        (name = "alerts", description = "Alerts and their history"),
        (name = "channels", description = "Notification channels"),
        (name = "silences", description = "Suppress notifications for a scope and time window"),
        (name = "push", description = "Web Push: VAPID key and this user's push subscriptions (devices)"),
        (name = "api-keys", description = "API keys for headless use"),
        (name = "system", description = "Health and the API description"),
    )
)]
pub struct ApiDoc;

/// The error body every failing request returns.
#[derive(Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    #[schema(value_type = ErrorCode)]
    pub code: String,
    /// Human-readable; never contains secrets.
    pub message: String,
}

/// Implemented error codes.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unauthorized,
    InvalidCredentials,
    Forbidden,
    CsrfFailed,
    WrongPassword,
    TooManyAttempts,
    NotFound,
    BadRequest,
    NameTaken,
    SyncInProgress,
    DefaultRule,
    AlertResolved,
    InvalidToken,
    EmailDisabled,
    PushDisabled,
    HetznerInvalidToken,
    HetznerUnavailable,
    HetznerRateLimited,
    Internal,
}

/// Schema-only string enums for fields stored as text, so generated clients get literal types.
pub mod enums {
    use serde::Serialize;
    use utoipa::ToSchema;

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Severity {
        Info,
        Warning,
        Critical,
    }

    /// Alert state.
    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum AlertState {
        Pending,
        Firing,
        Resolved,
    }

    /// `state` filter of `GET /alerts`; `open` = `pending` or `firing`.
    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum AlertFilter {
        Open,
        Pending,
        Firing,
        Resolved,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum AlertEventType {
        Pending,
        Fired,
        Resolved,
        Acked,
        Renotified,
        Suppressed,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum ResourceKind {
        Server,
        LoadBalancer,
        Volume,
        Certificate,
        FloatingIp,
        PrimaryIp,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum CheckType {
        ServerStatus,
        ResourceLifecycle,
        LbTargetHealth,
        TrafficQuota,
        HetznerApi,
        Cpu,
        Network,
        DiskIo,
        CertificateExpiry,
        Deprecation,
        HygieneBackups,
        HygieneProtection,
        HygieneOrphans,
        CostChange,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum ChannelType {
        Email,
        Slack,
        Discord,
        Webhook,
        Ntfy,
        WebPush,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum ProjectStatus {
        Active,
        InvalidToken,
        Degraded,
        Paused,
    }

    /// Delivery job status.
    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum JobStatus {
        Queued,
        Sent,
        Failed,
    }

    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Role {
        Owner,
        Admin,
        Member,
    }

    /// API key scope; `admin ⊃ write ⊃ read`.
    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum Scope {
        Read,
        Write,
        Admin,
    }

    /// How the request was authenticated.
    #[derive(Serialize, ToSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum AuthKind {
        Session,
        ApiKey,
    }
}

/// Completes the document once the router has added all paths: security schemes, per-operation security
/// and the shared auth error responses.
pub fn finish(openapi: &mut OpenApi) {
    describe_responses(openapi);
    if let Some(components) = openapi.components.as_mut() {
        for (name, schema) in &mut components.schemas {
            if !REQUEST_SCHEMAS.contains(&name.as_str()) {
                require_all(name, schema);
            }
        }
    }
    {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::Http(
                Http::builder()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("hm_…")
                    .description(Some("API key from `POST /api/v1/api-keys` or `bootstrap`"))
                    .build(),
            ),
        );
        components.add_security_scheme(
            "session",
            SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Cookie(
                utoipa::openapi::security::ApiKeyValue::with_description(
                    "hm_session",
                    "Session cookie set by `POST /api/v1/auth/login` (HttpOnly)",
                ),
            )),
        );
        components.add_security_scheme(
            "csrf",
            SecurityScheme::ApiKey(utoipa::openapi::security::ApiKey::Header(
                utoipa::openapi::security::ApiKeyValue::with_description(
                    "X-CSRF-Token",
                    "`csrf_token` from login or `GET /api/v1/me`; required with the session cookie \
                     for every method other than GET, HEAD and OPTIONS",
                ),
            )),
        );
        let unauthorized =
            auth_error("Missing, invalid, expired or revoked credentials (`unauthorized`)");
        let forbidden = auth_error(
            "The key or session lacks the required scope (`forbidden`), or `X-CSRF-Token` is missing or wrong \
             (`csrf_failed`)",
        );
        components
            .responses
            .insert("Unauthorized".to_owned(), RefOr::T(unauthorized));
        components
            .responses
            .insert("Forbidden".to_owned(), RefOr::T(forbidden));
    }
    {
        for (path, item) in &mut openapi.paths.paths {
            if PUBLIC.contains(&path.as_str()) {
                for op in operations(item) {
                    op.1.security = Some(Vec::new());
                }
                continue;
            }
            for (safe, op) in operations(item) {
                let cookie = if safe {
                    SecurityRequirement::new("session", Vec::<String>::new())
                } else {
                    SecurityRequirement::new("session", Vec::<String>::new())
                        .add("csrf", Vec::<String>::new())
                };
                op.security = Some(vec![
                    SecurityRequirement::new("api_key", Vec::<String>::new()),
                    cookie,
                ]);
                let responses = &mut op.responses.responses;
                responses.entry("401".to_owned()).or_insert_with(|| {
                    RefOr::Ref(utoipa::openapi::Ref::from_response_name("Unauthorized"))
                });
                responses.entry("403".to_owned()).or_insert_with(|| {
                    RefOr::Ref(utoipa::openapi::Ref::from_response_name("Forbidden"))
                });
            }
        }
    }
}

/// OpenAPI 3.1 requires a description on every response; default to the status text.
fn describe_responses(openapi: &mut OpenApi) {
    for item in openapi.paths.paths.values_mut() {
        for (_, op) in operations(item) {
            for (status, response) in &mut op.responses.responses {
                if let RefOr::T(response) = response
                    && response.description.is_empty()
                {
                    let reason = status
                        .parse::<u16>()
                        .ok()
                        .and_then(|s| axum::http::StatusCode::from_u16(s).ok())
                        .and_then(|s| s.canonical_reason())
                        .unwrap_or("Response");
                    reason.clone_into(&mut response.description);
                }
            }
        }
    }
}

/// Marks every property of a response object (or of the inline parts of an `allOf`) as required.
fn require_all(name: &str, schema: &mut RefOr<Schema>) {
    let RefOr::T(schema) = schema else { return };
    match schema {
        Schema::Object(object) => {
            for field in object.properties.keys() {
                let sparse = SPARSE_FIELDS.contains(&format!("{name}.{field}").as_str());
                if !sparse && !object.required.contains(field) {
                    object.required.push(field.clone());
                }
            }
        }
        Schema::AllOf(all) => {
            for item in &mut all.items {
                require_all(name, item);
            }
        }
        _ => {}
    }
}

/// The operations of a path item, with whether the method is CSRF-safe (GET/HEAD/OPTIONS).
fn operations(item: &mut utoipa::openapi::PathItem) -> Vec<(bool, &mut Operation)> {
    [
        (true, item.get.as_mut()),
        (true, item.head.as_mut()),
        (true, item.options.as_mut()),
        (false, item.post.as_mut()),
        (false, item.put.as_mut()),
        (false, item.patch.as_mut()),
        (false, item.delete.as_mut()),
    ]
    .into_iter()
    .filter_map(|(safe, op)| op.map(|op| (safe, op)))
    .collect()
}

fn auth_error(description: &str) -> Response {
    utoipa::openapi::ResponseBuilder::new()
        .description(description)
        .content(
            "application/json",
            utoipa::openapi::ContentBuilder::new()
                .schema(Some(utoipa::openapi::Ref::from_schema_name("ErrorBody")))
                .build(),
        )
        .build()
}
