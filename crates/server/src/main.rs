//! `hetzner-monitor` binary.
//!
//! Commands:
//! - `hetzner-monitor` / `hetzner-monitor serve` — run the service
//! - `hetzner-monitor healthcheck` — probe `/healthz` locally (for Docker `HEALTHCHECK`)

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use hm_server::config::{self, Config};
use hm_server::http::{AppState, router};
use hm_server::{db, telemetry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve().await,
        Some("healthcheck") => healthcheck().await,
        Some(other) => Err(anyhow::anyhow!(
            "unknown command `{other}`; expected `serve` or `healthcheck`"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Logging may not be initialised yet, so report on stderr.
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    telemetry::init(&config.log)?;
    tracing::info!(bind = %config.bind, mode = ?config.mode, version = env!("CARGO_PKG_VERSION"), "starting");

    let pool = db::connect(&config.database_url)
        .await
        .context("failed to connect to the database")?;
    db::migrate(&pool)
        .await
        .context("failed to apply migrations")?;
    tracing::info!("migrations applied");

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.bind))?;
    axum::serve(listener, router(AppState { db: pool.clone() }))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    pool.close().await;
    tracing::info!("stopped");
    Ok(())
}

/// Resolves on SIGINT or SIGTERM.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received");
}

/// Sends `GET /healthz` to the local listener and succeeds on HTTP 200.
async fn healthcheck() -> anyhow::Result<()> {
    let mut addr = config::bind_from_env()?;
    // A wildcard bind address is not connectable; probe loopback instead.
    if addr.ip().is_unspecified() {
        addr = match addr {
            SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::LOCALHOST, addr.port())),
            SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::LOCALHOST, addr.port())),
        };
    }
    let probe = async {
        let mut stream = TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
            .await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        Ok::<_, std::io::Error>(response)
    };
    let response = tokio::time::timeout(Duration::from_secs(3), probe)
        .await
        .context("healthcheck timed out")?
        .with_context(|| format!("healthcheck could not reach {addr}"))?;
    let status_line = response.split(|b| *b == b'\n').next().unwrap_or_default();
    if status_line.starts_with(b"HTTP/1.0 200") || status_line.starts_with(b"HTTP/1.1 200") {
        Ok(())
    } else {
        anyhow::bail!(
            "healthcheck failed: {}",
            String::from_utf8_lossy(status_line).trim()
        )
    }
}
