//! `hetzner-monitor` binary. Run `hetzner-monitor --help` for commands.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use hm_core::crypto::MasterKey;
use hm_server::config::{self, Config, Mode};
use hm_server::http::{AppState, router};
use hm_server::{bootstrap, db, scheduler, telemetry};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Monitoring & alerting for Hetzner Cloud.
#[derive(Parser)]
#[command(name = "hetzner-monitor", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the service (default).
    Serve,
    /// Probe `/healthz` on `HM_BIND` (for Docker `HEALTHCHECK`).
    Healthcheck,
    /// Create the first organisation, its owner and an admin API key (empty database only).
    Bootstrap {
        /// Organisation name.
        #[arg(long)]
        org: String,
        /// Owner e-mail address.
        #[arg(long)]
        email: String,
    },
    /// Master key utilities.
    MasterKey {
        #[command(subcommand)]
        action: MasterKeyAction,
    },
}

#[derive(Subcommand)]
enum MasterKeyAction {
    /// Print a new random master key (base64) for `HM_MASTER_KEY`.
    Generate,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve().await,
        Command::Healthcheck => healthcheck().await,
        Command::Bootstrap { org, email } => run_bootstrap(&org, &email).await,
        Command::MasterKey {
            action: MasterKeyAction::Generate,
        } => generate_master_key(),
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

    let state = AppState {
        db: pool.clone(),
        master_key: config.master_key.clone(),
        hcloud: hm_hcloud::Client::new().context("failed to build the Hetzner client")?,
    };
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.bind))?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let scheduler = matches!(config.mode, Mode::All | Mode::Worker)
        .then(|| tokio::spawn(scheduler::run(state.clone(), stop_rx)));
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;

    let _ = stop_tx.send(true);
    if let Some(handle) = scheduler {
        let _ = handle.await;
    }
    pool.close().await;
    tracing::info!("stopped");
    Ok(())
}

async fn run_bootstrap(org: &str, email: &str) -> anyhow::Result<()> {
    let database_url = config::database_url_from_env()?;
    let pool = db::connect(&database_url)
        .await
        .context("failed to connect to the database")?;
    db::migrate(&pool)
        .await
        .context("failed to apply migrations")?;
    let done = bootstrap::run(&pool, org, email).await?;
    // Operator-facing output on stdout, shown exactly once. Never logged.
    println!("Bootstrap complete.");
    println!("  organisation: {} ({})", org.trim(), done.org_id);
    println!("  owner:        {}", email.trim());
    println!();
    println!("Admin API key — shown only once, store it now (e.g. in your password manager):");
    println!("{}", done.key.full.expose().as_str());
    Ok(())
}

fn generate_master_key() -> anyhow::Result<()> {
    let key = MasterKey::generate_base64()?;
    println!("{}", key.as_str());
    eprintln!(
        "Store this as HM_MASTER_KEY (or in a file for HM_MASTER_KEY_FILE). Losing it makes stored tokens unrecoverable."
    );
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
