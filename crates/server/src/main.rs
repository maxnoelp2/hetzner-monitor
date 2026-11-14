//! `hetzner-monitor` binary. Run `hetzner-monitor --help` for commands.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand};
use hm_core::crypto::MasterKey;
use hm_server::config::{self, Config, Mode};
use hm_server::http::{AppState, router};
use hm_server::push::PushState;
use hm_server::{bootstrap, db, keys, notifier, scheduler, telemetry};
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
    /// Master key rotation (reads `HM_MASTER_KEY(S)(_FILE)` and `HM_DATABASE_URL`).
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },
    /// Print the OpenAPI 3.1 document (JSON) to stdout; needs no database or keys.
    Openapi,
    /// User administration.
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },
}

#[derive(Subcommand)]
enum KeysAction {
    /// Show how many stored secrets use each master key version.
    Status,
    /// Re-wrap all data keys with the newest configured master key version.
    Rotate,
}

#[derive(Subcommand)]
enum UsersAction {
    /// Set a new random password (printed once) and end all sessions of the user.
    ResetPassword {
        /// The user's e-mail address.
        #[arg(long)]
        email: String,
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
        Command::Keys { action } => run_keys(&action).await,
        Command::Openapi => print_openapi(),
        Command::Users {
            action: UsersAction::ResetPassword { email },
        } => reset_password(&email).await,
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
    keys::check_versions(&pool, &config.keyring)
        .await
        .context("master key check failed")?;

    let state = AppState {
        db: pool.clone(),
        keyring: config.keyring.clone(),
        hcloud: hm_hcloud::Client::new().context("failed to build the Hetzner client")?,
        notifier: hm_notify::Notifier::new(config.smtp.as_ref(), config.allow_private_targets)
            .context("failed to set up notifications")?,
        base_url: config.base_url.as_deref().map(Arc::from),
        login_throttle: Arc::default(),
        push: PushState::new(config.vapid_subject.as_deref()),
    };
    if state.push.enabled() {
        // Create the VAPID key pair now, so a broken key shows up at startup, not at the first alert.
        state
            .push
            .vapid(&state.db, &state.keyring)
            .await
            .context("web push setup failed")?;
        tracing::info!("web push enabled");
    } else {
        tracing::info!("web push disabled: set HM_VAPID_SUBJECT or an https:// HM_BASE_URL");
    }
    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("failed to bind {}", config.bind))?;

    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let worker = matches!(config.mode, Mode::All | Mode::Worker);
    let scheduler = worker.then(|| tokio::spawn(scheduler::run(state.clone(), stop_rx.clone())));
    let notifier = worker.then(|| tokio::spawn(notifier::run(state.clone(), stop_rx)));
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .context("HTTP server failed")?;

    let _ = stop_tx.send(true);
    for handle in [scheduler, notifier].into_iter().flatten() {
        let _ = handle.await;
    }
    pool.close().await;
    tracing::info!("stopped");
    Ok(())
}

async fn run_bootstrap(org: &str, email: &str) -> anyhow::Result<()> {
    let pool = connect_migrated().await?;
    let done = bootstrap::run(&pool, org, email).await?;
    // Operator-facing output on stdout, shown exactly once. Never logged.
    println!("Bootstrap complete.");
    println!("  organisation: {} ({})", org.trim(), done.org_id);
    println!("  owner:        {}", email.trim());
    println!();
    println!("Admin API key — shown only once, store it now (e.g. in your password manager):");
    println!("{}", done.key.full.expose().as_str());
    println!();
    println!("Initial password of the owner — shown only once; log in with it and change it:");
    println!("{}", done.password.expose().as_str());
    Ok(())
}

async fn connect_migrated() -> anyhow::Result<sqlx::PgPool> {
    let database_url = config::database_url_from_env()?;
    let pool = db::connect(&database_url)
        .await
        .context("failed to connect to the database")?;
    db::migrate(&pool)
        .await
        .context("failed to apply migrations")?;
    Ok(pool)
}

async fn run_keys(action: &KeysAction) -> anyhow::Result<()> {
    let keyring = config::keyring_from_env()?;
    let pool = connect_migrated().await?;
    if matches!(action, KeysAction::Rotate) {
        let done = keys::rotate(&pool, &keyring).await?;
        println!(
            "Re-wrapped {} project token(s), {} channel config(s), {} push subscription(s) and {} VAPID key(s) \
             to master key version {}.",
            done.projects,
            done.channels,
            done.push_subscriptions,
            done.vapid_keys,
            keyring.current_version()
        );
    }
    let status = keys::status(&pool).await?;
    println!(
        "Configured master key versions: {:?} (current: {})",
        keyring.versions().collect::<Vec<_>>(),
        keyring.current_version()
    );
    if status.rows.is_empty() {
        println!("No stored secrets.");
    }
    for ((table, version), count) in &status.rows {
        let note = if keyring.contains(*version) {
            ""
        } else {
            "  <- NOT CONFIGURED"
        };
        println!("  {table:<22} version {version}: {count} row(s){note}");
    }
    let old: Vec<i32> = status
        .versions()
        .filter(|v| *v != keyring.current_version())
        .collect();
    if old.is_empty() {
        println!("All secrets use the current version; older keys can be removed from the list.");
    } else {
        println!("Versions still in use besides the current one: {old:?} (run `keys rotate`).");
    }
    pool.close().await;
    Ok(())
}

async fn reset_password(email: &str) -> anyhow::Result<()> {
    let pool = connect_migrated().await?;
    let password = bootstrap::reset_password(&pool, email).await?;
    // Operator-facing output on stdout, shown exactly once. Never logged.
    println!(
        "New password for {} — shown only once; all their sessions were ended:",
        email.trim()
    );
    println!("{}", password.expose().as_str());
    pool.close().await;
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

fn print_openapi() -> anyhow::Result<()> {
    let json = hm_server::http::openapi().to_pretty_json()?;
    println!("{json}");
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
