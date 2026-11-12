//! Sending a [`Payload`] to a channel. Errors are classified as retryable or permanent and
//! carry short, redacted reasons — never a URL, token, secret or recipient.

use std::error::Error as _;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::stub::AsyncStubTransport;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};
use reqwest::{StatusCode, Url};

use crate::config::ChannelConfig;
use crate::guard::{self, Blocked};
use crate::payload::Payload;
use crate::render;

/// Timeout for one delivery attempt.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest `Retry-After` we honour.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);

/// Why a delivery failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// Try again later (timeouts, 5xx, 429, transient SMTP errors).
    #[error("{reason}")]
    Retry {
        reason: String,
        after: Option<Duration>,
    },
    /// Retrying will not help (4xx, invalid target, permanent SMTP errors, email not configured).
    #[error("{reason}")]
    Permanent { reason: String },
}

impl SendError {
    fn retry(reason: impl Into<String>) -> Self {
        Self::Retry {
            reason: reason.into(),
            after: None,
        }
    }

    fn permanent(reason: impl Into<String>) -> Self {
        Self::Permanent {
            reason: reason.into(),
        }
    }
}

/// How the SMTP connection is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// Plain connection upgraded with `STARTTLS` (required). Default port 587.
    Starttls,
    /// Implicit TLS. Default port 465.
    Tls,
    /// No encryption (local relays only). Default port 25.
    None,
}

/// Instance-wide SMTP settings (`HM_SMTP_*`).
#[derive(Clone)]
pub struct SmtpSettings {
    pub host: String,
    pub port: Option<u16>,
    pub tls: SmtpTls,
    pub user: Option<String>,
    pub password: Option<String>,
    pub from: String,
}

impl fmt::Debug for SmtpSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpSettings")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("user", &self.user.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("from", &self.from)
            .finish()
    }
}

/// Setup errors (bad SMTP settings, HTTP client).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SetupError(pub String);

#[derive(Clone)]
enum Mailer {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    Stub(AsyncStubTransport),
}

/// Sends notifications. Cheap to clone.
#[derive(Clone)]
pub struct Notifier {
    http: reqwest::Client,
    mail: Option<(Mailer, Mailbox)>,
    allow_private: bool,
}

impl fmt::Debug for Notifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Notifier")
            .field("email", &self.mail.is_some())
            .field("allow_private", &self.allow_private)
            .finish_non_exhaustive()
    }
}

impl Notifier {
    /// `smtp: None` disables email channels. `allow_private: false` blocks private targets.
    pub fn new(smtp: Option<&SmtpSettings>, allow_private: bool) -> Result<Self, SetupError> {
        let mail = smtp
            .map(|s| Ok::<_, SetupError>((Mailer::Smtp(smtp_transport(s)?), mailbox(&s.from)?)))
            .transpose()?;
        Ok(Self {
            http: http_client(allow_private)?,
            mail,
            allow_private,
        })
    }

    /// For tests: email goes to an in-memory stub transport (read it with `messages()`).
    pub fn with_stub_mail(
        from: &str,
        allow_private: bool,
    ) -> Result<(Self, AsyncStubTransport), SetupError> {
        let stub = AsyncStubTransport::new_ok();
        Ok((
            Self {
                http: http_client(allow_private)?,
                mail: Some((Mailer::Stub(stub.clone()), mailbox(from)?)),
                allow_private,
            },
            stub,
        ))
    }

    /// Whether email channels can be used.
    pub const fn email_enabled(&self) -> bool {
        self.mail.is_some()
    }

    /// Delivers `payload` to the channel. `delivery` is the job id (sent as `X-HM-Delivery`).
    pub async fn send(
        &self,
        config: &ChannelConfig,
        payload: &Payload,
        delivery: &str,
    ) -> Result<(), SendError> {
        match config {
            ChannelConfig::Email(c) => self.send_email(&c.to, payload).await,
            ChannelConfig::Slack(c) => {
                let body = serde_json::to_vec(&render::slack(payload)).unwrap_or_default();
                self.post(&c.url, body, &[]).await
            }
            ChannelConfig::Discord(c) => {
                let body = serde_json::to_vec(&render::discord(payload)).unwrap_or_default();
                self.post(&c.url, body, &[]).await
            }
            ChannelConfig::Webhook(c) => {
                let body = render::webhook(payload);
                let signature = render::signature(c.secret.as_bytes(), &body);
                let headers = [
                    ("X-HM-Event", payload.event.as_str().to_owned()),
                    ("X-HM-Delivery", delivery.to_owned()),
                    ("X-HM-Signature", signature),
                ];
                self.post(&c.url, body, &headers).await
            }
            ChannelConfig::Ntfy(c) => {
                let body = serde_json::to_vec(&render::ntfy(payload, &c.topic)).unwrap_or_default();
                let auth = c
                    .token
                    .as_ref()
                    .map(|t| ("Authorization", format!("Bearer {t}")));
                let headers: Vec<_> = auth.into_iter().collect();
                self.post(&c.server, body, &headers).await
            }
        }
    }

    async fn post(
        &self,
        url: &str,
        body: Vec<u8>,
        headers: &[(&str, String)],
    ) -> Result<(), SendError> {
        let url = Url::parse(url).map_err(|_| SendError::permanent("invalid target URL"))?;
        if !self.allow_private {
            // IP literals never reach the resolver.
            if let Some(ip) = url
                .host_str()
                .and_then(|h| h.trim_matches(['[', ']']).parse::<IpAddr>().ok())
                && guard::is_blocked(ip)
            {
                return Err(SendError::permanent("target address not allowed"));
            }
        }
        let mut request = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .body(body);
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let response = request.send().await.map_err(|e| classify(&e))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let reason = format!("HTTP {}", status.as_u16());
        if status == StatusCode::TOO_MANY_REQUESTS {
            let after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| Duration::from_secs_f64(s).min(MAX_RETRY_AFTER));
            return Err(SendError::Retry { reason, after });
        }
        if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
            return Err(SendError::retry(reason));
        }
        Err(SendError::permanent(reason))
    }

    async fn send_email(&self, to: &[String], payload: &Payload) -> Result<(), SendError> {
        let Some((mailer, from)) = &self.mail else {
            return Err(SendError::permanent(
                "email is not configured (HM_SMTP_HOST)",
            ));
        };
        let mut builder = Message::builder()
            .from(from.clone())
            .subject(payload.title())
            .header(ContentType::TEXT_PLAIN);
        for addr in to {
            let mailbox = addr
                .trim()
                .parse::<Mailbox>()
                .map_err(|_| SendError::permanent("invalid recipient address"))?;
            builder = builder.to(mailbox);
        }
        let message = builder
            .body(payload.text())
            .map_err(|_| SendError::permanent("could not build the email"))?;
        match mailer {
            Mailer::Smtp(t) => t.send(message).await.map(drop).map_err(|e| {
                if e.is_permanent() {
                    SendError::permanent("SMTP server rejected the message")
                } else if e.is_transient() {
                    SendError::retry("SMTP server deferred the message")
                } else {
                    SendError::retry("SMTP connection failed")
                }
            }),
            Mailer::Stub(t) => t
                .send(message)
                .await
                .map(drop)
                .map_err(|_| SendError::retry("stub transport failed")),
        }
    }
}

fn classify(err: &reqwest::Error) -> SendError {
    let mut source = err.source();
    while let Some(s) = source {
        if s.is::<Blocked>() {
            return SendError::permanent("target address not allowed");
        }
        source = s.source();
    }
    if err.is_timeout() {
        SendError::retry("timeout")
    } else if err.is_connect() {
        SendError::retry("connection failed")
    } else if err.is_redirect() {
        SendError::permanent("redirects are not followed")
    } else {
        SendError::retry("request failed")
    }
}

fn http_client(allow_private: bool) -> Result<reqwest::Client, SetupError> {
    // reqwest is built without a bundled crypto provider; installing fails harmlessly if one is set.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::Client::builder()
        .user_agent(concat!("hetzner-monitor/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(SEND_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if !allow_private {
        builder = builder.dns_resolver(guard::resolver());
    }
    builder
        .build()
        .map_err(|e| SetupError(format!("HTTP client: {}", e.without_url())))
}

fn mailbox(from: &str) -> Result<Mailbox, SetupError> {
    from.trim()
        .parse()
        .map_err(|_| SetupError("HM_SMTP_FROM is not a valid sender address".to_owned()))
}

fn smtp_transport(s: &SmtpSettings) -> Result<AsyncSmtpTransport<Tokio1Executor>, SetupError> {
    let tls_params = || {
        TlsParameters::new(s.host.clone())
            .map_err(|_| SetupError("HM_SMTP_HOST: TLS setup failed".to_owned()))
    };
    let (tls, default_port) = match s.tls {
        SmtpTls::Starttls => (Tls::Required(tls_params()?), 587),
        SmtpTls::Tls => (Tls::Wrapper(tls_params()?), 465),
        SmtpTls::None => (Tls::None, 25),
    };
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(s.host.clone())
        .port(s.port.unwrap_or(default_port))
        .tls(tls)
        .timeout(Some(SEND_TIMEOUT));
    if let (Some(user), Some(password)) = (&s.user, &s.password) {
        builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::payload::Event;
    use crate::payload::tests::sample;

    fn cfg(kind: &str, v: &serde_json::Value) -> ChannelConfig {
        ChannelConfig::from_parts(kind, v).unwrap()
    }

    #[tokio::test]
    async fn webhook_is_signed_and_classifies_responses() {
        let server = MockServer::start().await;
        let p = sample(Event::Fired);
        let body = render::webhook(&p);
        let sig = render::signature(b"0123456789abcdef", &body);
        Mock::given(method("POST"))
            .and(path("/ok"))
            .and(header("X-HM-Signature", sig.as_str()))
            .and(header("X-HM-Event", "fired"))
            .and(header("X-HM-Delivery", "42"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/busy"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "7"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/gone"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/down"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let n = Notifier::new(None, true).unwrap();
        let hook = |p: &str| {
            cfg(
                "webhook",
                &json!({ "url": format!("{}{p}", server.uri()), "secret": "0123456789abcdef" }),
            )
        };
        assert_eq!(n.send(&hook("/ok"), &p, "42").await, Ok(()));
        assert_eq!(
            n.send(&hook("/busy"), &p, "1").await,
            Err(SendError::Retry {
                reason: "HTTP 429".to_owned(),
                after: Some(Duration::from_secs(7))
            })
        );
        assert_eq!(
            n.send(&hook("/gone"), &p, "1").await,
            Err(SendError::permanent("HTTP 404"))
        );
        assert_eq!(
            n.send(&hook("/down"), &p, "1").await,
            Err(SendError::retry("HTTP 503"))
        );
    }

    #[tokio::test]
    async fn ntfy_sends_json_with_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", "Bearer tk_test"))
            .and(header_exists("content-type"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let n = Notifier::new(None, true).unwrap();
        let c = cfg(
            "ntfy",
            &json!({ "server": server.uri(), "topic": "alerts", "token": "tk_test" }),
        );
        n.send(&c, &sample(Event::Fired), "1").await.unwrap();
        let received = &server.received_requests().await.unwrap()[0];
        let v: serde_json::Value = serde_json::from_slice(&received.body).unwrap();
        assert_eq!(v["topic"], "alerts");
    }

    #[tokio::test]
    async fn private_targets_are_refused_when_disallowed() {
        let n = Notifier::new(None, false).unwrap();
        let p = sample(Event::Fired);
        for url in [
            "http://127.0.0.1:9/x",
            "http://169.254.169.254/latest",
            "http://[::1]:9/",
            "http://localhost:9/",
        ] {
            let c = cfg("slack", &json!({ "url": url }));
            assert_eq!(
                n.send(&c, &p, "1").await,
                Err(SendError::permanent("target address not allowed")),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn email_goes_through_the_transport() {
        let (n, stub) = Notifier::with_stub_mail("hm@example.com", true).unwrap();
        let c = cfg("email", &json!({ "to": ["ops@example.com"] }));
        n.send(&c, &sample(Event::Fired), "1").await.unwrap();
        let sent = stub.messages().await;
        assert_eq!(sent.len(), 1);
        assert!(
            sent[0]
                .1
                .contains("Subject: FIRING [critical] Server not running: server web-1")
        );
        let without = Notifier::new(None, true).unwrap();
        assert!(matches!(
            without.send(&c, &sample(Event::Fired), "1").await,
            Err(SendError::Permanent { .. })
        ));
    }
}
