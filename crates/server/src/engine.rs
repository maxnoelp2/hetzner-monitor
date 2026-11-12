//! Rule engine: evaluates a project's enabled rules after a poll cycle and persists alert
//! transitions (docs/design/alerting.md). Runs inside the cycle's transaction.
//!
//! - A **resource cycle** (fast / inventory) evaluates availability checks; inventory cycles also
//!   evaluate the inventory-only checks (certificates, deprecation, hygiene, cost, traffic
//!   forecast) over all six kinds. Fast cycles leave those alerts untouched.
//! - A **metrics cycle** evaluates metric checks (`cpu`, `network`, `disk_io`) on the series
//!   Hetzner just returned (nothing is stored, invariant 8).
//! - A **failed cycle** brought no data: only the `hetzner_api` system rule is evaluated; every
//!   other alert keeps its state (invariant 6).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use hm_core::alert::{self, Open, Transition};
use hm_core::checks::{self, CheckType, Context, Outcome};
use hm_core::delivery;
use hm_core::selector::Selector;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repo::alerts::{NewAlert, OpenAlert};
use crate::repo::resources::ResourceRow;
use crate::repo::rules::RuleView;
use crate::repo::{self, OrgId};

/// Kinds refreshed by the fast tier (and the metrics tier's servers).
const FAST_KINDS: [&str; 2] = ["server", "load_balancer"];
/// Kinds refreshed by the inventory tier.
const ALL_KINDS: [&str; 6] = [
    "server",
    "load_balancer",
    "volume",
    "certificate",
    "floating_ip",
    "primary_ip",
];
/// Age of the traffic sample a forecast extrapolates from (at least).
const FORECAST_BASELINE: time::Duration = time::Duration::seconds(checks::FORECAST_MIN_SPAN_S);

/// What the resource cycle changed.
pub struct Changes<'a> {
    pub created: &'a [Uuid],
    pub deleted: &'a [Uuid],
    /// False on a project's first successful sync: everything is "new" then, which is not news.
    pub lifecycle: bool,
    /// True for inventory cycles: all kinds are fresh, inventory-only checks are evaluated.
    pub inventory: bool,
}

/// Metric series of one server, as fetched by a metrics cycle.
pub struct ServerSeries {
    /// Resolution Hetzner used (seconds).
    pub step: f64,
    pub series: BTreeMap<String, Vec<(f64, f64)>>,
}

/// What a cycle brought.
pub enum Input<'a> {
    /// The cycle failed: no data.
    Failed,
    /// Fast or inventory cycle.
    Resources(&'a Changes<'a>),
    /// Metrics cycle: series per resource id. Servers without an entry were not fetched
    /// (e.g. deleted in the meantime) and keep their alerts unchanged.
    Metrics(&'a HashMap<Uuid, ServerSeries>),
}

/// The project as the cycle left it.
pub struct ProjectState<'a> {
    pub org: OrgId,
    pub id: Uuid,
    pub status: &'a str,
    pub message: Option<&'a str>,
}

/// Which servers the metrics tier has to fetch, and what.
#[derive(Debug, Default)]
pub struct MetricPlan {
    /// `(resource id, hetzner id)` of every active server in scope of an enabled metric rule.
    pub servers: Vec<(Uuid, i64)>,
    /// Hetzner metric families needed (`cpu`, `disk`, `network`).
    pub types: BTreeSet<&'static str>,
    /// Longest `for_s` of those rules (query window).
    pub longest_for_s: u64,
}

/// A rule's resource scope: optional single resource and optional label selector.
struct Scope {
    resource_id: Option<Uuid>,
    selector: Option<Selector>,
}

impl Scope {
    /// `None` (with a warning) if the rule cannot be evaluated.
    fn of(rule: &RuleView) -> Option<(CheckType, Self)> {
        let Ok(check) = rule.check_type.parse::<CheckType>() else {
            tracing::warn!(rule_id = %rule.id, check_type = %rule.check_type, "unknown check type, rule skipped");
            return None;
        };
        match rule
            .selector
            .as_deref()
            .map(str::parse::<Selector>)
            .transpose()
        {
            Ok(selector) => Some((
                check,
                Self {
                    resource_id: rule.resource_id,
                    selector,
                },
            )),
            Err(error) => {
                tracing::warn!(rule_id = %rule.id, %error, "invalid selector, rule skipped");
                None
            }
        }
    }

    fn contains(&self, r: &ResourceRow) -> bool {
        self.resource_id.is_none_or(|id| id == r.id)
            && self
                .selector
                .as_ref()
                .is_none_or(|s| s.matches(&r.label_map()))
    }
}

/// The servers and metric families the metrics tier needs for this project.
pub async fn metric_plan(
    conn: &mut PgConnection,
    org: OrgId,
    project_id: Uuid,
) -> Result<MetricPlan, sqlx::Error> {
    let rules = repo::rules::enabled_for_project(conn, org, project_id).await?;
    let mut plan = MetricPlan::default();
    let metric_rules: Vec<_> = rules
        .iter()
        .filter_map(|rule| Scope::of(rule).map(|(check, scope)| (rule, check, scope)))
        .filter(|(rule, check, _)| {
            check.metric_type().is_some() && check.validate_params(&rule.params.0).is_ok()
        })
        .collect();
    if metric_rules.is_empty() {
        return Ok(plan);
    }
    let servers = repo::resources::active(conn, project_id, &["server"]).await?;
    for server in &servers {
        let mut needed = false;
        for (rule, check, scope) in &metric_rules {
            if scope.contains(server) {
                needed = true;
                plan.types.extend(check.metric_type());
                plan.longest_for_s = plan.longest_for_s.max(check.window_s(&rule.params.0));
            }
        }
        if needed {
            plan.servers.push((server.id, server.hetzner_id));
        }
    }
    Ok(plan)
}

/// Evaluates the project's enabled rules for what the cycle brought.
pub async fn evaluate(
    conn: &mut PgConnection,
    project: &ProjectState<'_>,
    input: &Input<'_>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let rules = repo::rules::enabled_for_project(conn, project.org, project.id).await?;
    let mut open: HashMap<(Uuid, Option<Uuid>), OpenAlert> =
        repo::alerts::open_for_project(conn, project.org, project.id)
            .await?
            .into_iter()
            .map(|a| ((a.rule_id, a.resource_id), a))
            .collect();

    let inventory = matches!(input, Input::Resources(c) if c.inventory);
    let resources = match input {
        Input::Failed => Vec::new(),
        Input::Resources(_) if inventory => {
            repo::resources::active(conn, project.id, &ALL_KINDS).await?
        }
        Input::Resources(_) | Input::Metrics(_) => {
            repo::resources::active(conn, project.id, &FAST_KINDS).await?
        }
    };
    let baselines = if inventory {
        repo::snapshots::traffic_baselines(conn, project.org, project.id, now - FORECAST_BASELINE)
            .await?
    } else {
        HashMap::new()
    };
    let (created, deleted) = match input {
        Input::Resources(c) if c.lifecycle => (
            repo::resources::by_ids(conn, project.id, c.created).await?,
            repo::resources::by_ids(conn, project.id, c.deleted).await?,
        ),
        _ => (Vec::new(), Vec::new()),
    };

    for rule in &rules {
        let Some((check, scope)) = Scope::of(rule) else {
            continue;
        };
        let per_resource = check.kinds();
        if matches!(input, Input::Resources(_))
            && !inventory
            && check.inventory_only(&rule.params.0)
        {
            // Not fresh on a fast cycle: keep these alerts as they are.
            continue;
        }
        match (check, input) {
            (CheckType::HetznerApi, _) => {
                let outcome = api_outcome(project);
                let prev = open.remove(&(rule.id, None));
                apply(conn, rule, project.id, None, prev, &outcome, now).await?;
            }
            (CheckType::ResourceLifecycle, Input::Resources(_)) => {
                for (rows, was_created) in [(&created, true), (&deleted, false)] {
                    for r in rows.iter().filter(|r| scope.contains(r)) {
                        record_lifecycle(conn, rule, project.id, r, was_created, now).await?;
                    }
                }
            }
            (CheckType::CostChange, Input::Resources(_)) => {
                let prev = open.remove(&(rule.id, None));
                evaluate_cost(conn, project, rule, check, prev, now).await?;
            }
            (
                CheckType::ServerStatus
                | CheckType::LbTargetHealth
                | CheckType::TrafficQuota
                | CheckType::CertificateExpiry
                | CheckType::Deprecation
                | CheckType::HygieneBackups
                | CheckType::HygieneProtection
                | CheckType::HygieneOrphans,
                Input::Resources(_),
            ) => {
                let in_scope = resources
                    .iter()
                    .filter(|r| per_resource.contains(&r.kind.as_str()) && scope.contains(r));
                let eval = |r: &ResourceRow| {
                    let ctx = Context {
                        now: now.unix_timestamp(),
                        kind: &r.kind,
                        traffic_baseline: baselines.get(&r.id).copied(),
                    };
                    check.evaluate(&rule.params.0, &r.snapshot.0, &ctx)
                };
                evaluate_each(conn, rule, project.id, in_scope, eval, &mut open, now).await?;
            }
            (CheckType::Cpu | CheckType::Network | CheckType::DiskIo, Input::Metrics(series)) => {
                let in_scope = resources
                    .iter()
                    .filter(|r| per_resource.contains(&r.kind.as_str()) && scope.contains(r));
                evaluate_metrics(
                    conn, rule, check, project.id, in_scope, series, &mut open, now,
                )
                .await?;
            }
            // A failed cycle evaluates nothing else (invariant 6); other checks belong to
            // the other kind of cycle.
            _ => {}
        }
    }
    Ok(())
}

/// The project-level `cost_change` rule on today's estimate (written by this inventory cycle)
/// and the latest estimate of an earlier day. Without today's estimate the alert is left alone.
async fn evaluate_cost(
    conn: &mut PgConnection,
    project: &ProjectState<'_>,
    rule: &RuleView,
    check: CheckType,
    prev: Option<OpenAlert>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let (current, previous) =
        repo::snapshots::cost_points(conn, project.org, project.id, now.date()).await?;
    let Some(current) = current else {
        return Ok(());
    };
    match check.evaluate_cost(&rule.params.0, &current, previous.as_ref()) {
        Ok(Some(outcome)) => apply(conn, rule, project.id, None, prev, &outcome, now).await,
        Ok(None) => Ok(()),
        Err(error) => {
            tracing::warn!(rule_id = %rule.id, %error, "invalid params, rule skipped");
            Ok(())
        }
    }
}

/// A metric rule on the series of this metrics cycle. Servers in scope that were not fetched
/// keep their alerts as they are.
#[allow(clippy::too_many_arguments)]
async fn evaluate_metrics<'r>(
    conn: &mut PgConnection,
    rule: &RuleView,
    check: CheckType,
    project_id: Uuid,
    in_scope: impl Iterator<Item = &'r ResourceRow>,
    series: &HashMap<Uuid, ServerSeries>,
    open: &mut HashMap<(Uuid, Option<Uuid>), OpenAlert>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    #[allow(clippy::cast_precision_loss)]
    let now_s = now.unix_timestamp() as f64;
    let mut fetched = Vec::new();
    for r in in_scope {
        if series.contains_key(&r.id) {
            fetched.push(r);
        } else {
            // Not fetched this cycle: leave its alert as it is.
            open.remove(&(rule.id, Some(r.id)));
        }
    }
    let eval = |r: &ResourceRow| {
        let data = &series[&r.id];
        check.evaluate_metrics(
            &rule.params.0,
            &r.snapshot.0,
            &data.series,
            data.step,
            now_s,
        )
    };
    evaluate_each(conn, rule, project_id, fetched.into_iter(), eval, open, now).await
}

/// Applies one rule to each resource; closes the rule's alerts of resources not evaluated
/// (deleted / out of scope). Invalid params skip the rule and leave its alerts alone.
async fn evaluate_each<'r>(
    conn: &mut PgConnection,
    rule: &RuleView,
    project_id: Uuid,
    resources: impl Iterator<Item = &'r ResourceRow>,
    eval: impl Fn(&ResourceRow) -> Result<Option<Outcome>, checks::ParamsError>,
    open: &mut HashMap<(Uuid, Option<Uuid>), OpenAlert>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    for r in resources {
        let outcome = match eval(r) {
            Ok(Some(outcome)) => outcome,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(rule_id = %rule.id, %error, "invalid params, rule skipped");
                return Ok(());
            }
        };
        let prev = open.remove(&(rule.id, Some(r.id)));
        apply(conn, rule, project_id, Some(r.id), prev, &outcome, now).await?;
    }
    close_gone(conn, rule, open, now).await
}

/// A `resource_lifecycle` event: an alert that fires and resolves at once.
async fn record_lifecycle(
    conn: &mut PgConnection,
    rule: &RuleView,
    project_id: Uuid,
    r: &ResourceRow,
    created: bool,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let message = checks::lifecycle_message(&r.kind, &r.name, r.hetzner_id, created);
    let alert = NewAlert {
        rule_id: rule.id,
        resource_id: Some(r.id),
        project_id,
        state: "resolved",
        severity: &rule.severity,
        value: None,
        message: &message,
    };
    repo::alerts::insert(conn, &alert, now).await?;
    Ok(())
}

fn api_outcome(project: &ProjectState<'_>) -> Outcome {
    let down = project.status != "active";
    let message = if down {
        let reason = project.message.unwrap_or("no details");
        match project.status {
            "invalid_token" => format!("Hetzner rejected the project's API token: {reason}"),
            status => format!("Hetzner API {status}: {reason}"),
        }
    } else {
        "Hetzner API reachable".to_owned()
    };
    Outcome {
        condition: down,
        value: None,
        message,
        hold_s: 0,
        held_s: None,
    }
}

/// Open alerts of `rule` whose resource was not evaluated this cycle (deleted, or no longer
/// in scope) are closed.
async fn close_gone(
    conn: &mut PgConnection,
    rule: &RuleView,
    open: &mut HashMap<(Uuid, Option<Uuid>), OpenAlert>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let keys: Vec<_> = open
        .keys()
        .filter(|(r, _)| *r == rule.id)
        .copied()
        .collect();
    for key in keys {
        if let Some(prev) = open.remove(&key) {
            let message = "resource deleted or no longer in the rule's scope";
            match alert::gone(open_state(&prev)) {
                Transition::Resolve => {
                    repo::alerts::resolve(conn, prev.id, None, message, now).await?;
                }
                _ => repo::alerts::cancel(conn, prev.id).await?,
            }
        }
    }
    Ok(())
}

fn open_state(a: &OpenAlert) -> Open {
    if a.state == "pending" {
        Open::Pending {
            since: a.pending_since.unix_timestamp(),
        }
    } else {
        Open::Firing {
            ok_cycles: u32::try_from(a.ok_cycles).unwrap_or(0),
        }
    }
}

async fn apply(
    conn: &mut PgConnection,
    rule: &RuleView,
    project_id: Uuid,
    resource_id: Option<Uuid>,
    prev: Option<OpenAlert>,
    outcome: &Outcome,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let transition = alert::step(
        prev.as_ref().map(open_state),
        outcome.condition,
        outcome.hold_s,
        outcome.held_s,
        now.unix_timestamp(),
    );
    let new = |state| NewAlert {
        rule_id: rule.id,
        resource_id,
        project_id,
        state,
        severity: &rule.severity,
        value: outcome.value,
        message: &outcome.message,
    };
    match (transition, prev) {
        (Transition::OpenPending, _) => {
            repo::alerts::insert(conn, &new("pending"), now).await?;
        }
        (Transition::OpenFiring, _) => {
            let id = repo::alerts::insert(conn, &new("firing"), now).await?;
            tracing::info!(alert_id = %id, rule_id = %rule.id, message = %outcome.message, "alert firing");
        }
        (Transition::StayPending, Some(prev)) => {
            repo::alerts::touch(conn, prev.id, outcome.value, &outcome.message, 0, now).await?;
        }
        (Transition::StayFiring { ok_cycles }, Some(prev)) => {
            repo::alerts::touch(
                conn,
                prev.id,
                outcome.value,
                &outcome.message,
                ok_cycles,
                now,
            )
            .await?;
            // Only while the condition still holds, not during the flapping guard's good cycles.
            let last = prev.last_notified_at.or(prev.fired_at);
            if ok_cycles == 0
                && delivery::renotify_due(
                    &rule.severity,
                    prev.acked_at.is_some(),
                    last.map(OffsetDateTime::unix_timestamp),
                    i64::from(rule.renotify_s),
                    now.unix_timestamp(),
                )
            {
                repo::alerts::renotify(conn, prev.id, outcome.value, &outcome.message, now).await?;
            }
        }
        (Transition::Fire, Some(prev)) => {
            repo::alerts::fire(conn, prev.id, outcome.value, &outcome.message, now).await?;
            tracing::info!(alert_id = %prev.id, rule_id = %rule.id, message = %outcome.message, "alert firing");
        }
        (Transition::Cancel, Some(prev)) => repo::alerts::cancel(conn, prev.id).await?,
        (Transition::Resolve, Some(prev)) => {
            repo::alerts::resolve(conn, prev.id, outcome.value, &outcome.message, now).await?;
            tracing::info!(alert_id = %prev.id, rule_id = %rule.id, message = %outcome.message, "alert resolved");
        }
        // Transitions on an open alert always come with one.
        (Transition::Nothing, _) | (_, None) => {}
    }
    Ok(())
}
