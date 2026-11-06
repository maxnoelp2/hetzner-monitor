/// Errors from the Hetzner Cloud API, mapped per `docs/design/hcloud-client.md`.
/// Messages never contain the token.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    /// 401 `unauthorized`: the token is invalid or was revoked.
    #[error("Hetzner rejected the API token")]
    InvalidToken,
    /// 403 `forbidden`: the token lacks permission.
    #[error("Hetzner denied access: {message}")]
    Forbidden { message: String },
    /// 403 `maintenance`.
    #[error("Hetzner API is in maintenance: {message}")]
    Maintenance { message: String },
    /// 429: rate limit exhausted.
    #[error("Hetzner rate limit exceeded")]
    RateLimited { reset_at: Option<i64> },
    /// 5xx, timeouts, connection errors.
    #[error("Hetzner API unavailable: {message}")]
    Unavailable {
        status: Option<u16>,
        message: String,
    },
    /// Any other non-success response.
    #[error("Hetzner API error {status} ({code}): {message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    /// The response did not have the expected shape.
    #[error("unexpected Hetzner API response: {0}")]
    Decode(String),
    /// The HTTP client could not be built.
    #[error("HTTP client setup failed: {0}")]
    Setup(String),
}
