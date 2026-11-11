//! Scheduler (docs/design/scheduler.md): a central ticker claims due projects with a lease and
//! runs one poll cycle per claimed project. Several instances (`HM_MODE=worker`) can run side
//! by side; the lease makes sure a project is never polled twice at the same time.

use std::time::Duration as StdDuration;

use time::{Duration, OffsetDateTime};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::cycle::{self, CycleError, Tier};
use crate::http::AppState;
use crate::repo::{self, projects::Claimed};

/// How often the ticker looks for due projects.
pub const TICK: StdDuration = StdDuration::from_secs(5);
/// How long a claim is valid. Bounds a cycle; a dead worker's projects are free again after it.
pub const LEASE: Duration = Duration::minutes(5);
/// Upper bound of concurrently running cycles per instance.
pub const MAX_IN_FLIGHT: usize = 16;
/// How long shutdown waits for running cycles.
const SHUTDOWN_GRACE: StdDuration = StdDuration::from_secs(25);

/// Runs until `shutdown` turns true. Cycles run concurrently; a slow project does not delay others.
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    tracing::info!(tick_s = TICK.as_secs(), "scheduler started");
    let mut in_flight = JoinSet::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                while in_flight.try_join_next().is_some() {}
                if let Err(error) = spawn_due(&state, OffsetDateTime::now_utc(), &mut in_flight).await {
                    tracing::warn!(%error, "scheduler could not claim projects");
                }
            }
            _ = shutdown.changed() => break,
        }
    }
    tracing::info!(running = in_flight.len(), "scheduler stopping");
    let drain = async { while in_flight.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, drain).await.is_err() {
        tracing::warn!(
            "scheduler stopped with cycles still running; their leases expire on their own"
        );
    }
}

/// One tick for tests and tools: claims everything due at `now` and waits for the cycles.
/// Returns the number of cycles run.
pub async fn tick(state: &AppState, now: OffsetDateTime) -> Result<usize, sqlx::Error> {
    let mut set = JoinSet::new();
    let n = spawn_due(state, now, &mut set).await?;
    while set.join_next().await.is_some() {}
    Ok(n)
}

async fn spawn_due(
    state: &AppState,
    now: OffsetDateTime,
    set: &mut JoinSet<()>,
) -> Result<usize, sqlx::Error> {
    let free = MAX_IN_FLIGHT.saturating_sub(set.len());
    if free == 0 {
        return Ok(0);
    }
    let claimed = repo::projects::claim_due(
        &state.db,
        now,
        now + LEASE,
        i64::try_from(free).unwrap_or(i64::MAX),
    )
    .await?;
    let n = claimed.len();
    for project in claimed {
        let state = state.clone();
        set.spawn(async move { run_claimed(&state, &project, now).await });
    }
    Ok(n)
}

/// Runs every due tier of a claimed project in order, then releases the lease. A failing
/// tier stops the rest (its failure already postponed all tiers).
async fn run_claimed(state: &AppState, project: &Claimed, now: OffsetDateTime) {
    let Ok(token) = cycle::open_token(state, project) else {
        // Keep the lease: retried when it expires, without hammering the log.
        tracing::error!(project_id = %project.id, "stored token could not be decrypted (wrong HM_MASTER_KEY?)");
        return;
    };
    for tier in Tier::due(project, now) {
        match cycle::run(state, project, &token, tier, now).await {
            Ok(_) => {}
            Err(CycleError::Hetzner(_)) => break,
            Err(error) => {
                tracing::error!(project_id = %project.id, ?tier, %error, "poll cycle failed");
                break;
            }
        }
    }
    if let Err(error) = repo::projects::release(&state.db, project.org(), project.id).await {
        tracing::warn!(project_id = %project.id, %error, "could not release lease");
    }
}
