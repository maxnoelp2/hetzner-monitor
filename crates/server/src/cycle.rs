//! One poll cycle of a project: fetch the tier's data from Hetzner, apply it, update schedule
//! and budget, and run the rule engine — all in one transaction. Used by the scheduler, by
//! connect (first sync) and by manual sync. Callers hold the project's lease and release it.

use std::collections::{BTreeMap, HashMap};

use hm_core::{budget, crypto, series};
use hm_hcloud::{MetricType, RateLimit, RawResource, ResourceKind, Token};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::engine::{self, Changes, Input, ProjectState, ServerSeries};
use crate::http::AppState;
use crate::repo::projects::{Claimed, Failure, Success};
use crate::repo::resources::KindCounts;
use crate::repo::{self, OrgId};

/// Per-kind counts of one cycle, keyed by kind (`server`, …).
pub type SyncReport = BTreeMap<&'static str, KindCounts>;

/// Consecutive failures after which a project counts as `degraded`.
pub const DEGRADED_AFTER: i32 = 3;

/// What a cycle fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Servers (and load balancers, if the project has any).
    Fast,
    /// All six resource kinds.
    Inventory,
    /// Metric series of the servers in scope of metric rules.
    Metrics,
}

impl Tier {
    /// The tiers due for a claimed project, in the order they run: inventory (which includes
    /// the fast tier's data) or fast, then metrics.
    pub fn due(project: &Claimed, now: OffsetDateTime) -> Vec<Self> {
        let due = |at: Option<OffsetDateTime>| at.is_none_or(|at| at <= now);
        let mut tiers = Vec::with_capacity(2);
        if due(project.next_inventory_at) {
            tiers.push(Self::Inventory);
        } else if due(project.next_fast_at) {
            tiers.push(Self::Fast);
        }
        if project.next_metrics_at.is_some_and(|at| at <= now) {
            tiers.push(Self::Metrics);
        }
        tiers
    }
}

/// Why a cycle failed.
#[derive(Debug, thiserror::Error)]
pub enum CycleError {
    /// Hetzner answered with an error or was unreachable; already recorded on the project.
    #[error(transparent)]
    Hetzner(hm_hcloud::Error),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    /// The stored token could not be decrypted (wrong master key?).
    #[error("token could not be decrypted")]
    Token,
}

/// Decrypts a claimed project's token.
pub fn open_token(state: &AppState, project: &Claimed) -> Result<Token, CycleError> {
    let plain = crypto::open(&state.master_key, project.id.as_bytes(), &project.sealed())
        .map_err(|_| CycleError::Token)?;
    let token = String::from_utf8(plain.to_vec()).map_err(|_| CycleError::Token)?;
    Ok(Token::new(token))
}

/// Runs one cycle. Hetzner errors are recorded on the project (status, backoff, system
/// alert) before they are returned. A metrics cycle returns an empty report.
pub async fn run(
    state: &AppState,
    project: &Claimed,
    token: &Token,
    tier: Tier,
    now: OffsetDateTime,
) -> Result<SyncReport, CycleError> {
    if tier == Tier::Metrics {
        run_metrics(state, project, token, now).await?;
        return Ok(SyncReport::new());
    }
    let org = project.org();
    let kinds = kinds_for(state, project.id, tier).await?;

    let mut fetched: Vec<(ResourceKind, Vec<RawResource>)> = Vec::with_capacity(kinds.len());
    let mut rate_limit = RateLimit::default();
    for kind in kinds {
        match state.hcloud.list(token, kind).await {
            Ok(listed) => {
                rate_limit = listed.rate_limit;
                fetched.push((kind, listed.items));
            }
            Err(err) => {
                tracing::warn!(project_id = %project.id, kind = %kind, error = %err, "poll cycle failed");
                record_failure(state, project, &err, now).await?;
                return Err(CycleError::Hetzner(err));
            }
        }
    }

    let mut tx = state.db.begin().await?;
    let mut report = SyncReport::new();
    let (mut created, mut deleted) = (Vec::new(), Vec::new());
    for (kind, items) in &fetched {
        let applied = repo::resources::apply(&mut tx, project.id, *kind, items, now).await?;
        report.insert(kind.as_str(), applied.counts);
        created.extend(applied.created);
        deleted.extend(applied.deleted);
    }

    let counts = repo::resources::counts_by_kind(&mut tx, project.id).await?;
    let count = |kind: ResourceKind| {
        counts
            .iter()
            .find(|(k, _)| k == kind.as_str())
            .map_or(0, |(_, n)| u32::try_from(*n).unwrap_or(u32::MAX))
    };
    let (servers, lbs) = (
        count(ResourceKind::Server),
        count(ResourceKind::LoadBalancer),
    );
    let per_kind: Vec<u32> = ResourceKind::ALL.into_iter().map(count).collect();
    let budget_h = budget::budget_h(rate_limit.limit, f64::from(project.budget_share));
    let fixed_h = budget::fixed_h(
        budget::fast_cost(servers, lbs),
        budget::inventory_cost(&per_kind),
    );
    let inventory = tier == Tier::Inventory;
    let plan = engine::metric_plan(&mut tx, org, project.id).await?;
    let n_metrics = u32::try_from(plan.servers.len()).unwrap_or(u32::MAX);
    let metrics_interval_s = budget::metrics_interval_s(
        n_metrics,
        budget_h,
        fixed_h,
        rate_limit.remaining,
        rate_limit.limit,
    );
    let success = Success {
        rate_limit,
        inventory,
        next_fast_at: now + Duration::seconds(i64::from(budget::FAST_INTERVAL_S) + jitter_s()),
        next_inventory_at: inventory
            .then(|| now + Duration::seconds(i64::from(budget::INVENTORY_INTERVAL_S))),
        budget_h,
        fixed_h,
        metrics_interval_s,
        metrics_from: metrics_interval_s.map(|_| now + Duration::seconds(jitter_s())),
    };
    repo::projects::record_success(&mut tx, org, project.id, now, &success).await?;

    let changes = Changes {
        created: &created,
        deleted: &deleted,
        lifecycle: project.last_inventory_at.is_some(),
    };
    let project_state = ProjectState {
        org,
        id: project.id,
        status: "active",
        message: None,
    };
    engine::evaluate(&mut tx, &project_state, &Input::Resources(&changes), now).await?;
    tx.commit().await?;
    tracing::debug!(project_id = %project.id, ?tier, "poll cycle done");
    Ok(report)
}

/// Metrics cycle: one request per server in scope of a metric rule, evaluated on the series
/// right away. A server that vanished since the last inventory (404) is skipped.
async fn run_metrics(
    state: &AppState,
    project: &Claimed,
    token: &Token,
    now: OffsetDateTime,
) -> Result<(), CycleError> {
    let org = project.org();
    let plan = {
        let mut conn = state.db.acquire().await?;
        engine::metric_plan(&mut conn, org, project.id).await?
    };
    let budget_h = project.budget_h.and_then(|v| u32::try_from(v).ok());
    let fixed_h = project.fixed_h.and_then(|v| u32::try_from(v).ok());
    let n = u32::try_from(plan.servers.len()).unwrap_or(u32::MAX);
    let interval = |rl: RateLimit| match (budget_h, fixed_h) {
        (Some(b), Some(f)) => budget::metrics_interval_s(n, b, f, rl.remaining, rl.limit),
        // Not computed yet (no successful resource cycle): be conservative.
        _ => (n > 0).then_some(budget::INVENTORY_INTERVAL_S),
    };
    let types: Vec<MetricType> = plan
        .types
        .iter()
        .filter_map(|t| match *t {
            "cpu" => Some(MetricType::Cpu),
            "disk" => Some(MetricType::Disk),
            "network" => Some(MetricType::Network),
            _ => None,
        })
        .collect();

    let mut fetched = HashMap::with_capacity(plan.servers.len());
    let mut rate_limit = RateLimit::default();
    if !types.is_empty() {
        let planned = interval(RateLimit::default()).unwrap_or(budget::MIN_METRICS_INTERVAL_S);
        let (start, end) = series::window(now.unix_timestamp(), planned, plan.longest_for_s);
        let start = OffsetDateTime::from_unix_timestamp(start).unwrap_or(now);
        let end = OffsetDateTime::from_unix_timestamp(end).unwrap_or(now);
        for (resource_id, hetzner_id) in &plan.servers {
            match state
                .hcloud
                .server_metrics(token, *hetzner_id, &types, start, end, series::STEP_S)
                .await
            {
                Ok(metrics) => {
                    rate_limit = metrics.rate_limit;
                    fetched.insert(
                        *resource_id,
                        ServerSeries {
                            step: metrics.step,
                            series: metrics.series,
                        },
                    );
                }
                Err(hm_hcloud::Error::Api { status: 404, .. }) => {
                    tracing::debug!(project_id = %project.id, server = hetzner_id, "server gone, metrics skipped");
                }
                Err(err) => {
                    tracing::warn!(project_id = %project.id, server = hetzner_id, error = %err, "metrics cycle failed");
                    record_failure(state, project, &err, now).await?;
                    return Err(CycleError::Hetzner(err));
                }
            }
        }
    }

    let interval_s = interval(rate_limit);
    let next = interval_s.map(|s| now + Duration::seconds(i64::from(s) + jitter_s()));
    let mut tx = state.db.begin().await?;
    repo::projects::record_metrics_success(
        &mut tx, org, project.id, now, rate_limit, interval_s, next,
    )
    .await?;
    let project_state = ProjectState {
        org,
        id: project.id,
        status: "active",
        message: None,
    };
    engine::evaluate(&mut tx, &project_state, &Input::Metrics(&fetched), now).await?;
    tx.commit().await?;
    tracing::debug!(project_id = %project.id, servers = fetched.len(), "metrics cycle done");
    Ok(())
}

/// Kinds to fetch: the fast tier skips load balancers while the project has none.
async fn kinds_for(
    state: &AppState,
    project_id: Uuid,
    tier: Tier,
) -> Result<Vec<ResourceKind>, sqlx::Error> {
    if tier == Tier::Inventory {
        return Ok(ResourceKind::ALL.to_vec());
    }
    let mut conn = state.db.acquire().await?;
    let counts = repo::resources::counts_by_kind(&mut conn, project_id).await?;
    let has_lbs = counts
        .iter()
        .any(|(k, n)| k == ResourceKind::LoadBalancer.as_str() && *n > 0);
    Ok(if has_lbs {
        vec![ResourceKind::Server, ResourceKind::LoadBalancer]
    } else {
        vec![ResourceKind::Server]
    })
}

/// Records a failed cycle: status, backoff, and the `hetzner_api` system alert.
async fn record_failure(
    state: &AppState,
    project: &Claimed,
    err: &hm_hcloud::Error,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let org: OrgId = project.org();
    let message = err.to_string();
    // A 429 means nothing is left until the reset; other errors carry no usable headers.
    let rate_limit = match err {
        hm_hcloud::Error::RateLimited { reset_at } => RateLimit {
            limit: None,
            remaining: Some(0),
            reset_at: *reset_at,
        },
        _ => RateLimit::default(),
    };
    let (status, failures, retry_at) = match err {
        hm_hcloud::Error::InvalidToken => (
            "invalid_token",
            project.failures.saturating_add(1),
            now + Duration::seconds(i64::from(budget::FAST_INTERVAL_S)),
        ),
        // The project's budget is used up elsewhere; not a fault of the API.
        hm_hcloud::Error::RateLimited { reset_at } => {
            let earliest = now + Duration::seconds(i64::from(budget::FAST_INTERVAL_S));
            let reset = reset_at
                .and_then(|ts| OffsetDateTime::from_unix_timestamp(ts).ok())
                .unwrap_or(earliest);
            (
                project.status.as_str(),
                project.failures,
                reset.max(earliest),
            )
        }
        _ => {
            let failures = project.failures.saturating_add(1);
            let status = if failures >= DEGRADED_AFTER {
                "degraded"
            } else {
                project.status.as_str()
            };
            let wait = budget::backoff_s(u32::try_from(failures).unwrap_or(u32::MAX));
            (status, failures, now + Duration::seconds(i64::from(wait)))
        }
    };
    let mut tx = state.db.begin().await?;
    let failure = Failure {
        status,
        message: &message,
        failures,
        rate_limit,
        retry_at,
    };
    repo::projects::record_failure(&mut tx, org, project.id, &failure).await?;
    let project_state = ProjectState {
        org,
        id: project.id,
        status,
        message: Some(&message),
    };
    engine::evaluate(&mut tx, &project_state, &Input::Failed, now).await?;
    tx.commit().await
}

/// 0–5 s, so projects don't poll in lockstep.
fn jitter_s() -> i64 {
    let mut byte = [0_u8; 1];
    if getrandom::fill(&mut byte).is_err() {
        return 0;
    }
    i64::from(byte[0] % 6)
}
