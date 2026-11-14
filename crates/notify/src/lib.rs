//! Notification channel adapters: email (SMTP), Slack, Discord, signed webhook, ntfy and Web
//! Push (one message per subscribed device, [`push`]), behind one [`Notifier`].
//!
//! See `docs/design/alerting.md#notifications`. This crate depends on nothing internal: the
//! server builds a [`Payload`] and decrypts the [`ChannelConfig`]; this crate renders and sends.

pub mod config;
pub mod guard;
pub mod payload;
pub mod push;
pub mod render;
pub mod send;

pub use config::{ChannelConfig, ConfigError};
pub use payload::{AlertInfo, Event, Named, PAYLOAD_VERSION, Payload, ResourceInfo, Severity};
pub use send::{Notifier, SendError, SetupError, SmtpSettings, SmtpTls};

/// In-memory mail transport for tests ([`Notifier::with_stub_mail`]).
pub use lettre::transport::stub::AsyncStubTransport as StubMail;
