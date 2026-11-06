//! Configuration from `HM_*` environment variables. See `docs/design/configuration.md`.

use std::net::SocketAddr;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use hm_core::crypto::MasterKey;
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
    /// Envelope-encryption master key (`HM_MASTER_KEY` or `HM_MASTER_KEY_FILE`).
    pub master_key: Arc<MasterKey>,
    pub bind: SocketAddr,
    pub log: String,
    pub mode: Mode,
}

impl Config {
    /// Reads the configuration from the process environment and file system.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(
            |key| std::env::var(key).ok(),
            |path| std::fs::read_to_string(path),
        )
    }

    /// Reads the configuration through `lookup` (environment variables) and `read_file`
    /// (for `*_FILE` variables).
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&Path) -> std::io::Result<String>,
    ) -> Result<Self, ConfigError> {
        let database_url = database_url_from_lookup(&lookup)?;
        let mode = match lookup("HM_MODE") {
            Some(value) => value.parse().map_err(|reason| ConfigError::Invalid {
                var: "HM_MODE",
                reason,
            })?,
            None => Mode::All,
        };
        Ok(Self {
            database_url,
            master_key: Arc::new(master_key_from_lookup(&lookup, &read_file)?),
            bind: bind_from_lookup(&lookup)?,
            log: lookup("HM_LOG").unwrap_or_else(|| DEFAULT_LOG.to_owned()),
            mode,
        })
    }
}

/// Reads only `HM_DATABASE_URL` (used by CLI commands that need no master key).
pub fn database_url_from_env() -> Result<Secret<String>, ConfigError> {
    database_url_from_lookup(&|key: &str| std::env::var(key).ok())
}

fn database_url_from_lookup(
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<Secret<String>, ConfigError> {
    lookup("HM_DATABASE_URL")
        .filter(|value| !value.trim().is_empty())
        .map(Secret::new)
        .ok_or(ConfigError::Missing("HM_DATABASE_URL"))
}

fn master_key_from_lookup(
    lookup: &impl Fn(&str) -> Option<String>,
    read_file: &impl Fn(&Path) -> std::io::Result<String>,
) -> Result<MasterKey, ConfigError> {
    let inline = lookup("HM_MASTER_KEY").filter(|value| !value.trim().is_empty());
    let file = lookup("HM_MASTER_KEY_FILE").filter(|value| !value.trim().is_empty());
    let (var, encoded) = match (inline, file) {
        (Some(_), Some(_)) => {
            return Err(ConfigError::Invalid {
                var: "HM_MASTER_KEY",
                reason: "set either HM_MASTER_KEY or HM_MASTER_KEY_FILE, not both".to_owned(),
            });
        }
        (Some(value), None) => ("HM_MASTER_KEY", Secret::new(value)),
        (None, Some(path)) => {
            let content = read_file(Path::new(&path)).map_err(|err| ConfigError::Invalid {
                var: "HM_MASTER_KEY_FILE",
                reason: format!("cannot read `{path}`: {err}"),
            })?;
            ("HM_MASTER_KEY_FILE", Secret::new(content))
        }
        (None, None) => return Err(ConfigError::Missing("HM_MASTER_KEY")),
    };
    MasterKey::from_base64(encoded.expose()).map_err(|err| ConfigError::Invalid {
        var,
        reason: format!("{err}; generate one with `hetzner-monitor master-key generate`"),
    })
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

    use std::path::Path;

    use super::{Config, ConfigError, Mode};

    const KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    fn lookup(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let mut map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        // A valid master key unless the test sets the key variables itself.
        if !map.contains_key("HM_MASTER_KEY")
            && !map.contains_key("HM_MASTER_KEY_FILE")
            && !map.contains_key("NO_KEY")
        {
            map.insert("HM_MASTER_KEY".to_owned(), KEY.to_owned());
        }
        move |key| map.get(key).cloned()
    }

    fn no_files(_: &Path) -> std::io::Result<String> {
        Err(std::io::Error::other("no files in this test"))
    }

    fn load(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
        Config::from_lookup(lookup(vars), no_files)
    }

    #[test]
    fn master_key_is_required_and_validated() {
        assert_eq!(
            load(&[("HM_DATABASE_URL", "x"), ("NO_KEY", "1")]).unwrap_err(),
            ConfigError::Missing("HM_MASTER_KEY")
        );
        let err = load(&[("HM_DATABASE_URL", "x"), ("HM_MASTER_KEY", "c2hvcnQ=")]).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "HM_MASTER_KEY",
                ..
            }
        ));
        let err = load(&[
            ("HM_DATABASE_URL", "x"),
            ("HM_MASTER_KEY", KEY),
            ("HM_MASTER_KEY_FILE", "/k"),
        ])
        .unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "HM_MASTER_KEY",
                ..
            }
        ));
    }

    #[test]
    fn master_key_can_come_from_a_file() {
        let config = Config::from_lookup(
            lookup(&[
                ("HM_DATABASE_URL", "x"),
                ("HM_MASTER_KEY_FILE", "/run/secrets/key"),
            ]),
            |path| {
                assert_eq!(path, Path::new("/run/secrets/key"));
                Ok(format!("{KEY}\n"))
            },
        );
        assert!(config.is_ok());
        let err =
            load(&[("HM_DATABASE_URL", "x"), ("HM_MASTER_KEY_FILE", "/missing")]).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                var: "HM_MASTER_KEY_FILE",
                ..
            }
        ));
    }

    #[test]
    fn defaults_apply_when_only_database_url_is_set() {
        let config = load(&[("HM_DATABASE_URL", "postgres://db/app")]).unwrap();
        assert_eq!(config.bind.to_string(), "0.0.0.0:8080");
        assert_eq!(config.log, "info");
        assert_eq!(config.mode, Mode::All);
        assert_eq!(config.database_url.expose(), "postgres://db/app");
    }

    #[test]
    fn missing_or_empty_database_url_is_an_error() {
        assert_eq!(
            load(&[]).unwrap_err(),
            ConfigError::Missing("HM_DATABASE_URL")
        );
        assert_eq!(
            load(&[("HM_DATABASE_URL", "  ")]).unwrap_err(),
            ConfigError::Missing("HM_DATABASE_URL")
        );
    }

    #[test]
    fn invalid_mode_and_bind_are_rejected() {
        let err = load(&[("HM_DATABASE_URL", "x"), ("HM_MODE", "both")]).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "HM_MODE", .. }));
        let err = load(&[("HM_DATABASE_URL", "x"), ("HM_BIND", "nope")]).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { var: "HM_BIND", .. }));
    }

    #[test]
    fn all_modes_parse() {
        for (raw, mode) in [
            ("all", Mode::All),
            ("api", Mode::Api),
            ("worker", Mode::Worker),
        ] {
            let config = load(&[("HM_DATABASE_URL", "x"), ("HM_MODE", raw)]).unwrap();
            assert_eq!(config.mode, mode);
        }
    }

    #[test]
    fn debug_output_redacts_database_url() {
        let config = load(&[("HM_DATABASE_URL", "postgres://u:hunter2@db/app")]).unwrap();
        assert!(!format!("{config:?}").contains("hunter2"));
    }
}
