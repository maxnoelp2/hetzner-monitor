//! Rule engine: evaluates a project's enabled rules after a poll cycle and persists alert
//! transitions (docs/design/alerting.md). Runs inside the cycle's transaction.
//!
//! Invariant 6: when the cycle brought no data (Hetzner unreachable, token invalid), only the
//! `hetzner_api` system rule is evaluated; availability alerts keep their state.

use std::collections::HashMap;

use hm_core::alert::{self, Open, Transition};
use hm_core::checks::{self, CheckType, Outcome};
use hm_core::selector::Selector;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::repo::alerts::{NewAlert, OpenAlert};
use crate::repo::resources::ResourceRow;
use crate::repo::rules::RuleView;
use crate::repo::{self, OrgId};

/// Kinds that per-resource checks look at; refreshed by both tiers.
const EVALUATED_KINDS: [&str; 2] = ["server", "load_balancer"];

/// What the cycle changed, if it fetched data.
pub struct Changes<'a> {
    pub created: &'a [Uuid],
    pub deleted: &'a [Uuid],
    /// False on a project's first successful sync: everything is "new" then, which is not news.
    pub lifecycle: bool,
}

/// The project as the cycle left it.
pub struct ProjectState<'a> {
    pub org: OrgId,
    pub id: Uuid,
    pub status: &'a str,
    pub message: Option<&'a str>,
}

/// Evaluates all enabled rules of the project. `changes` is `None` if the cycle failed.
pub async fn evaluate(
    conn: &mut PgConnection,
    project: &ProjectState<'_>,
    changes: Option<&Changes<'_>>,
    now: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let rules = repo::rules::enabled_for_project(conn, project.org, project.id).await?;
    let mut open: HashMap<(Uuid, Option<Uuid>), OpenAlert> =
        repo::alerts::open_for_project(conn, project.org, project.id)
            .await?
            .into_iter()
            .map(|a| ((a.rule_id, a.resource_id), a))
            .collect();

    let (resources, created, deleted) = match changes {
        Some(c) => (
            repo::resources::active(conn, project.id, &EVALUATED_KINDS).await?,
            if c.lifecycle {
                repo::resources::by_ids(conn, project.id, c.created).await?
            } else {
                Vec::new()
            },
            if c.lifecycle {
                repo::resources::by_ids(conn, project.id, c.deleted).await?
            } else {
                Vec::new()
            },
        ),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };

    for rule in &rules {
        let Ok(check) = rule.check_type.parse::<CheckType>() else {
            tracing::warn!(rule_id = %rule.id, check_type = %rule.check_type, "unknown check type, rule skipped");
            continue;
        };
        let selector = match rule
            .selector
            .as_deref()
            .map(str::parse::<Selector>)
            .transpose()
        {
            Ok(selector) => selector,
            Err(error) => {
                tracing::warn!(rule_id = %rule.id, %error, "invalid selector, rule skipped");
                continue;
            }
        };
        let in_scope = |r: &ResourceRow| {
            rule.resource_id.is_none_or(|id| id == r.id)
                && selector.as_ref().is_none_or(|s| s.matches(&r.label_map()))
        };

        match check {
            CheckType::HetznerApi => {
                let outcome = api_outcome(project);
                let prev = open.remove(&(rule.id, None));
                apply(conn, rule, project.id, None, prev, &outcome, now).await?;
            }
            // No fresh data: availability checks are suspended (invariant 6).
            _ if changes.is_none() => {}
            CheckType::ResourceLifecycle => {
                for (rows, was_created) in [(&created, true), (&deleted, false)] {
                    for r in rows.iter().filter(|r| in_scope(r)) {
                        let message =
                            checks::lifecycle_message(&r.kind, &r.name, r.hetzner_id, was_created);
                        let alert = NewAlert {
                            rule_id: rule.id,
                            resource_id: Some(r.id),
                            project_id: project.id,
                            state: "resolved",
                            severity: &rule.severity,
                            value: None,
                            message: &message,
                        };
                        repo::alerts::insert(conn, &alert, now).await?;
                    }
                }
            }
            CheckType::ServerStatus | CheckType::LbTargetHealth | CheckType::TrafficQuota => {
                let mut evaluated = true;
                for r in resources
                    .iter()
                    .filter(|r| check.kinds().contains(&r.kind.as_str()) && in_scope(r))
                {
                    let outcome = match check.evaluate(&rule.params.0, &r.snapshot.0) {
                        Ok(Some(outcome)) => outcome,
                        Ok(None) => continue,
                        Err(error) => {
                            tracing::warn!(rule_id = %rule.id, %error, "invalid params, rule skipped");
                            evaluated = false;
                            break;
                        }
                    };
                    let prev = open.remove(&(rule.id, Some(r.id)));
                    apply(conn, rule, project.id, Some(r.id), prev, &outcome, now).await?;
                }
                if evaluated {
                    close_gone(conn, rule, &mut open, now).await?;
                }
            }
        }
    }
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
