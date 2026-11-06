//! Configuration from `HM_*` environment variables. See `docs/design/configuration.md`.

use std::net::SocketAddr;
use std::str::FromStr;

use hm_core::secret::Secret;

/// Default listen address for the HTTP API.
pub const DEFAULT_BIND: &str = "0.0.0.0:8080";
/// Default `tracing` filter.
pub const DEFAULT_LOG: &str = "info";

/// Which subsystems this process runs (ADR-0002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// API and worker in one process (default, self-hosted).
    All,
    /// HTTP API only.
    Api,
    /// Scheduler and notifier only.
    Worker,
}

impl FromStr for Mode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "all" => Ok(Self::All),
            "api" => Ok(Self::Api),
            "worker" => Ok(Self::Worker),
            other => Err(format!("expected `all`, `api` or `worker`, got `{other}`")),
        }
    }
}

/// Errors while reading configuration. Never contains secret values.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("required environment variable {0} is not set")]
    Missing(&'static str),
    #[error("environment variable {var} is invalid: {reason}")]
    Invalid { var: &'static str, reason: String },
}

/// Service configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres connection string (contains credentials, hence secret).
    pub database_url: Secret<String>,
    pub bind: SocketAddr,
    pub log: String,
    pub mode: Mode,
}

impl Config {
    /// Reads the configuration from the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    /// Reads the configuration through `lookup`, which returns the value of a variable if set.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let database_url = lookup("HM_DATABASE_URL")
            .filter(|value| !value.trim().is_empty())
            .ok_or(ConfigError::Missing("HM_DATABASE_URL"))?;
        let mode = match lookup("HM_MODE") {
            Some(value) => value.parse().map_err(|reason| ConfigError::Invalid {
                var: "HM_MODE",
                reason,
            })?,
            None => Mode::All,
        };
        Ok(Self {
            database_url: Secret::new(database_url),
            bind: bind_from_lookup(&lookup)?,
            log: lookup("HM_LOG").unwrap_or_else(|| DEFAULT_LOG.to_owned()),
            mode,
        })
    }
}

/// Reads only `HM_BIND` (used by the `healthcheck` subcommand, which needs no database).
pub fn bind_from_env() -> Result<SocketAddr, ConfigError> {
    bind_from_lookup(&|key: &str| std::env::var(key).ok())
}

fn bind_from_lookup(lookup: &impl Fn(&str) -> Option<String>) -> Result<SocketAddr, ConfigError> {
    let raw = lookup("HM_BIND").unwrap_or_else(|| DEFAULT_BIND.to_owned());
    raw.parse().map_err(|err| ConfigError::Invalid {
        var: "HM_BIND",
        reason: format!("`{raw}` is not a socket address ({err})"),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{Config, ConfigError, Mode};

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_apply_when_only_database_url_is_set() {
        let config =
            Config::from_lookup(lookup(&[("HM_DATABASE_URL", "postgres://db/app")])).unwrap();
        assert_eq!(config.bind.to_string(), "0.0.0.0:8080");
        assert_eq!(config.log, "info");
        assert_eq!(config.mode, Mode::All);
        assert_eq!(config.database_url.expose(), "postgres://db/app");
    }

    #[test]
    fn missing_or_empty_database_url_is_an_error() {
        assert_eq!(
            Config::from_lookup(lookup(&[])).unwrap_err(),
            ConfigError::Missing("HM_DATABASE_URL")
        );
        assert_eq!(
            Config::from_lookup(lookup(&[("HM_DATABASE_URL", "  ")])).unwrap_err(),
            ConfigError::Missing("HM_DATABASE_URL")
        );
    }

    #[test]
    fn invalid_mode_and_bind_are_rejected() {
        let err = Config::from_lookup(lookup(&[("HM_DATABASE_URL", "x"), ("HM_MODE", "both")]))
            .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "HM_MODE", .. }));
        let err = Config::from_lookup(lookup(&[("HM_DATABASE_URL", "x"), ("HM_BIND", "nope")]))
            .unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "HM_BIND", .. }));
    }

    #[test]
    fn all_modes_parse() {
        for (raw, mode) in [
            ("all", Mode::All),
            ("api", Mode::Api),
            ("worker", Mode::Worker),
        ] {
            let config =
                Config::from_lookup(lookup(&[("HM_DATABASE_URL", "x"), ("HM_MODE", raw)])).unwrap();
            assert_eq!(config.mode, mode);
        }
    }

    #[test]
    fn debug_output_redacts_database_url() {
        let config = Config::from_lookup(lookup(&[(
            "HM_DATABASE_URL",
            "postgres://u:hunter2@db/app",
        )]))
        .unwrap();
        assert!(!format!("{config:?}").contains("hunter2"));
    }
}
