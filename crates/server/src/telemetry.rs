//! Structured JSON logging via `tracing`.

use tracing_subscriber::EnvFilter;

/// Installs the global JSON log subscriber with the given filter (`HM_LOG`, `RUST_LOG` syntax).
pub fn init(filter: &str) -> anyhow::Result<()> {
    let filter =
        EnvFilter::try_new(filter).map_err(|err| anyhow::anyhow!("HM_LOG is invalid: {err}"))?;
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_current_span(false)
        .try_init()
        .map_err(|err| anyhow::anyhow!("failed to install log subscriber: {err}"))
}
