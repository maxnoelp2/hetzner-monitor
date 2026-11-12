//! Channel configurations. They contain secrets (webhook URLs, tokens, signing secrets,
//! recipients) and are stored envelope-encrypted; `Debug` never shows their content.

use std::fmt;

use lettre::Address;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Default ntfy server.
pub const NTFY_DEFAULT_SERVER: &str = "https://ntfy.sh";
/// Most recipients of one email channel.
pub const MAX_RECIPIENTS: usize = 20;

/// The channel types.
pub const KINDS: [&str; 5] = ["email", "slack", "discord", "webhook", "ntfy"];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Email {
    pub to: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hook {
    pub url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    pub url: String,
    /// HMAC-SHA256 key for `X-HM-Signature`.
    pub secret: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ntfy {
    #[serde(default = "default_ntfy_server")]
    pub server: String,
    pub topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

fn default_ntfy_server() -> String {
    NTFY_DEFAULT_SERVER.to_owned()
}

/// A validated channel configuration.
#[derive(Clone)]
pub enum ChannelConfig {
    Email(Email),
    Slack(Hook),
    Discord(Hook),
    Webhook(Webhook),
    Ntfy(Ntfy),
}

/// Why a configuration was rejected. Never contains the secret values themselves.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

fn invalid(msg: impl Into<String>) -> ConfigError {
    ConfigError(msg.into())
}

impl ChannelConfig {
    /// Parses and validates `config` for channel type `kind` (API input and stored JSON).
    pub fn from_parts(kind: &str, config: &Value) -> Result<Self, ConfigError> {
        // serde errors may quote values; report only the field-level problem.
        fn parse<T: for<'de> Deserialize<'de>>(kind: &str, v: &Value) -> Result<T, ConfigError> {
            T::deserialize(v).map_err(|e| {
                let text = e.to_string();
                let reason =
                    if text.starts_with("unknown field") || text.starts_with("missing field") {
                        text.split(", expected").next().unwrap_or(&text).to_owned()
                    } else {
                        "invalid value".to_owned()
                    };
                invalid(format!("{kind} config: {reason}"))
            })
        }
        let cfg = match kind {
            "email" => Self::Email(parse(kind, config)?),
            "slack" => Self::Slack(parse(kind, config)?),
            "discord" => Self::Discord(parse(kind, config)?),
            "webhook" => Self::Webhook(parse(kind, config)?),
            "ntfy" => Self::Ntfy(parse(kind, config)?),
            other => {
                return Err(invalid(format!(
                    "type must be one of {}, got `{other}`",
                    KINDS.join(", ")
                )));
            }
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Email(_) => "email",
            Self::Slack(_) => "slack",
            Self::Discord(_) => "discord",
            Self::Webhook(_) => "webhook",
            Self::Ntfy(_) => "ntfy",
        }
    }

    /// The JSON stored (encrypted) for this config.
    pub fn to_json(&self) -> Value {
        let v = match self {
            Self::Email(c) => serde_json::to_value(c),
            Self::Slack(c) | Self::Discord(c) => serde_json::to_value(c),
            Self::Webhook(c) => serde_json::to_value(c),
            Self::Ntfy(c) => serde_json::to_value(c),
        };
        v.unwrap_or(Value::Null)
    }

    /// A non-secret description for the API: the target host or the recipient count.
    pub fn hint(&self) -> String {
        match self {
            Self::Email(c) => match c.to.len() {
                1 => "1 recipient".to_owned(),
                n => format!("{n} recipients"),
            },
            Self::Slack(c) | Self::Discord(c) => host(&c.url),
            Self::Webhook(c) => host(&c.url),
            Self::Ntfy(c) => host(&c.server),
        }
    }

    /// The HTTP target URL (none for email).
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Email(_) => None,
            Self::Slack(c) | Self::Discord(c) => Some(&c.url),
            Self::Webhook(c) => Some(&c.url),
            Self::Ntfy(c) => Some(&c.server),
        }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        match self {
            Self::Email(c) => {
                if c.to.is_empty() || c.to.len() > MAX_RECIPIENTS {
                    return Err(invalid(format!(
                        "email config: `to` needs 1 to {MAX_RECIPIENTS} addresses"
                    )));
                }
                for (i, addr) in c.to.iter().enumerate() {
                    addr.trim().parse::<Address>().map_err(|_| {
                        invalid(format!("email config: `to[{i}]` is not an email address"))
                    })?;
                }
                Ok(())
            }
            Self::Slack(c) | Self::Discord(c) => valid_url(self.kind(), "url", &c.url),
            Self::Webhook(c) => {
                valid_url("webhook", "url", &c.url)?;
                if !(16..=256).contains(&c.secret.len()) {
                    return Err(invalid(
                        "webhook config: `secret` must be 16 to 256 characters",
                    ));
                }
                Ok(())
            }
            Self::Ntfy(c) => {
                valid_url("ntfy", "server", &c.server)?;
                let ok = (1..=64).contains(&c.topic.len())
                    && c.topic
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
                if !ok {
                    return Err(invalid(
                        "ntfy config: `topic` must be 1 to 64 characters of A-Z, a-z, 0-9, _ or -",
                    ));
                }
                if c.token.as_deref().is_some_and(str::is_empty) {
                    return Err(invalid("ntfy config: `token` must not be empty"));
                }
                Ok(())
            }
        }
    }
}

impl fmt::Debug for ChannelConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChannelConfig::{}([REDACTED])", self.kind())
    }
}

fn valid_url(kind: &str, field: &str, raw: &str) -> Result<(), ConfigError> {
    let url =
        Url::parse(raw).map_err(|_| invalid(format!("{kind} config: `{field}` is not a URL")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid(format!(
            "{kind} config: `{field}` must be an http(s) URL with a host"
        )));
    }
    Ok(())
}

fn host(raw: &str) -> String {
    Url::parse(raw)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_every_kind_and_hints_without_secrets() {
        let slack = ChannelConfig::from_parts(
            "slack",
            &json!({ "url": "https://hooks.slack.com/services/T0/B0/secretpart" }),
        )
        .unwrap();
        assert_eq!(slack.hint(), "hooks.slack.com");
        assert_eq!(format!("{slack:?}"), "ChannelConfig::slack([REDACTED])");

        let email = ChannelConfig::from_parts(
            "email",
            &json!({ "to": ["ops@example.com", "b@example.com"] }),
        )
        .unwrap();
        assert_eq!(email.hint(), "2 recipients");

        let ntfy = ChannelConfig::from_parts("ntfy", &json!({ "topic": "hm-alerts_1" })).unwrap();
        assert_eq!(ntfy.hint(), "ntfy.sh");
        assert_eq!(
            ntfy.to_json(),
            json!({ "server": "https://ntfy.sh", "topic": "hm-alerts_1" })
        );

        let hook = ChannelConfig::from_parts(
            "webhook",
            &json!({ "url": "https://example.com/hook", "secret": "0123456789abcdef" }),
        )
        .unwrap();
        // Round trip through the stored JSON.
        let again = ChannelConfig::from_parts("webhook", &hook.to_json()).unwrap();
        assert_eq!(again.hint(), "example.com");
    }

    #[test]
    fn rejects_invalid_configs_without_echoing_values() {
        let cases = [
            ("slack", json!({ "url": "ftp://x.example" })),
            ("slack", json!({ "url": "not a url sekrit" })),
            ("discord", json!({})),
            (
                "webhook",
                json!({ "url": "https://x.example", "secret": "short" }),
            ),
            ("ntfy", json!({ "topic": "bad topic!" })),
            ("email", json!({ "to": [] })),
            ("email", json!({ "to": ["nope"] })),
            ("email", json!({ "to": ["a@example.com"], "cc": ["x"] })),
            ("pager", json!({})),
        ];
        for (kind, cfg) in cases {
            let err = ChannelConfig::from_parts(kind, &cfg).unwrap_err();
            assert!(!err.0.contains("sekrit"), "{err}");
        }
    }
}
