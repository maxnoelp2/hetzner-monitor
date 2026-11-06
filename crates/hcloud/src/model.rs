use std::fmt;
use std::str::FromStr;

use serde_json::{Map, Value};
use zeroize::Zeroizing;

/// A Hetzner Cloud API token. Never printed; dropped memory is wiped.
#[derive(Clone)]
pub struct Token(Zeroizing<String>);

impl Token {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The plaintext, for encrypting it at rest. Do not use it for anything else.
    #[must_use]
    pub fn reveal_for_storage(&self) -> Zeroizing<String> {
        self.0.clone()
    }

    /// Last four characters, safe to show to the owner as a hint.
    #[must_use]
    pub fn hint(&self) -> String {
        let chars: Vec<char> = self.0.chars().collect();
        chars[chars.len().saturating_sub(4)..].iter().collect()
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token([REDACTED])")
    }
}

/// Resource kinds discovered during inventory sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Server,
    LoadBalancer,
    Volume,
    Certificate,
    FloatingIp,
    PrimaryIp,
}

impl ResourceKind {
    pub const ALL: [Self; 6] = [
        Self::Server,
        Self::LoadBalancer,
        Self::Volume,
        Self::Certificate,
        Self::FloatingIp,
        Self::PrimaryIp,
    ];

    /// Stable identifier used in the database and the API (`server`, `load_balancer`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::LoadBalancer => "load_balancer",
            Self::Volume => "volume",
            Self::Certificate => "certificate",
            Self::FloatingIp => "floating_ip",
            Self::PrimaryIp => "primary_ip",
        }
    }

    /// Hetzner list endpoint, which is also the JSON key of the item array.
    #[must_use]
    pub const fn collection(self) -> &'static str {
        match self {
            Self::Server => "servers",
            Self::LoadBalancer => "load_balancers",
            Self::Volume => "volumes",
            Self::Certificate => "certificates",
            Self::FloatingIp => "floating_ips",
            Self::PrimaryIp => "primary_ips",
        }
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ResourceKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| format!("unknown resource kind `{value}`"))
    }
}

/// Rate-limit state reported by the last response (`RateLimit-*` headers).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: Option<u32>,
    pub remaining: Option<u32>,
    /// UNIX timestamp (seconds) when the limit is fully recovered.
    pub reset_at: Option<i64>,
}

/// A listed resource: the fields every kind shares plus the raw API object.
#[derive(Debug, Clone, PartialEq)]
pub struct RawResource {
    pub id: i64,
    pub name: String,
    pub labels: Map<String, Value>,
    pub raw: Value,
}

/// Result of a list call: all pages combined, plus the rate limit after the last page.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed<T> {
    pub items: Vec<T>,
    pub rate_limit: RateLimit,
}

#[cfg(test)]
mod tests {
    use super::{ResourceKind, Token};

    #[test]
    fn token_is_redacted_and_hint_is_last_four() {
        let token = Token::new("abcdefgh1234".to_owned());
        assert_eq!(format!("{token:?}"), "Token([REDACTED])");
        assert_eq!(token.hint(), "1234");
        assert_eq!(Token::new("ab".to_owned()).hint(), "ab");
    }

    #[test]
    fn kinds_round_trip() {
        for kind in ResourceKind::ALL {
            assert_eq!(kind.as_str().parse::<ResourceKind>().unwrap(), kind);
        }
        assert!("router".parse::<ResourceKind>().is_err());
    }
}
