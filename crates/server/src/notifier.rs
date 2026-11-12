//! Notifier (ADR-0012): sends queued notification jobs with retry and backoff. Runs next to the
//! scheduler in modes `all` and `worker`. Each job is sent while its row is locked
//! (`SKIP LOCKED`), so several instances never send the same job at the same time.

use std::time::Duration as StdDuration;

use hm_core::crypto::{self, Sealed};
use hm_core::delivery;
use hm_notify::{
    AlertInfo, ChannelConfig, Event, Named, PAYLOAD_VERSION, Payload, ResourceInfo, SendError,
};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use tokio::sync::watch;

use crate::http::AppState;
use crate::repo::notifications::{self, Job};

/// How often the notifier looks for due jobs.
pub const TICK: StdDuration = StdDuration::from_secs(2);
/// Most jobs sent per tick (the next tick continues).
pub const MAX_PER_TICK: usize = 50;
/// How often finished jobs are cleaned up.
const CLEANUP_EVERY: StdDuration = StdDuration::from_secs(3600);

/// Runs until `shutdown` turns true. A job being sent finishes first (bounded by the send timeout).
pub async fn run(state: AppState, mut shutdown: watch::Receiver<bool>) {
    tracing::info!(tick_s = TICK.as_secs(), "notifier started");
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut cleanup = tokio::time::interval(CLEANUP_EVERY);
    cleanup.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = run_once(&state, OffsetDateTime::now_utc()).await {
                    tracing::warn!(%error, "notifier could not process jobs");
                }
            }
            _ = cleanup.tick() => {
                match notifications::cleanup(&state.db, OffsetDateTime::now_utc()).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(deleted = n, "old notification jobs cleaned up"),
                    Err(error) => tracing::warn!(%error, "notification job cleanup failed"),
                }
            }
            _ = shutdown.changed() => break,
        }
    }
    tracing::info!("notifier stopped");
}

/// Sends every job due at `now` (up to [`MAX_PER_TICK`]); for tests with a synthetic clock.
/// Returns the number of jobs processed (sent, re-queued or failed).
pub async fn run_once(state: &AppState, now: OffsetDateTime) -> Result<usize, sqlx::Error> {
    let mut done = 0;
    while done < MAX_PER_TICK {
        let mut tx = state.db.begin().await?;
        let Some(job) = notifications::claim_next(&mut tx, now).await? else {
            break;
        };
        let result = deliver(state, &job).await;
        let attempts = u32::try_from(job.attempts).unwrap_or(0) + 1;
        match result {
            Ok(()) => {
                notifications::mark_sent(&mut tx, job.id, now).await?;
                tracing::info!(job_id = job.id, channel_id = %job.channel_id, event = %job.event_type, "notification sent");
            }
            Err(error) => {
                let (reason, retry_at) = match &error {
                    SendError::Retry { reason, after } if attempts < delivery::MAX_ATTEMPTS => {
                        let wait = delivery::backoff_s(attempts, after.map(|d| d.as_secs()));
                        (
                            reason.as_str(),
                            Some(now + Duration::seconds(i64::try_from(wait).unwrap_or(3600))),
                        )
                    }
                    SendError::Retry { reason, .. } | SendError::Permanent { reason } => {
                        (reason.as_str(), None)
                    }
                };
                notifications::mark_failed(&mut tx, job.id, reason, retry_at, now).await?;
                if retry_at.is_some() {
                    tracing::info!(job_id = job.id, channel_id = %job.channel_id, attempts, reason, "notification retry scheduled");
                } else {
                    tracing::warn!(job_id = job.id, channel_id = %job.channel_id, attempts, reason, "notification failed");
                }
            }
        }
        tx.commit().await?;
        done += 1;
    }
    Ok(done)
}

async fn deliver(state: &AppState, job: &Job) -> Result<(), SendError> {
    if !job.channel_enabled {
        return Err(permanent("channel disabled"));
    }
    let config = decrypt_config(
        state,
        job.channel_id,
        &job.channel_type,
        &Sealed {
            secret_ct: job.config_ct.clone(),
            dek_ct: job.config_dek_ct.clone(),
            key_version: job.key_version,
        },
    )?;
    let payload = payload(state, job).map_err(|e| permanent(&e))?;
    state
        .notifier
        .send(&config, &payload, &job.id.to_string())
        .await
}

/// Opens and parses a channel config (AAD = channel id). Errors are redacted.
pub fn decrypt_config(
    state: &AppState,
    channel_id: uuid::Uuid,
    kind: &str,
    sealed: &Sealed,
) -> Result<ChannelConfig, SendError> {
    let plain = crypto::open(&state.keyring, channel_id.as_bytes(), sealed)
        .map_err(|_| permanent("channel config cannot be decrypted"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&plain).map_err(|_| permanent("channel config is corrupt"))?;
    ChannelConfig::from_parts(kind, &value).map_err(|_| permanent("channel config is invalid"))
}

fn permanent(reason: &str) -> SendError {
    SendError::Permanent {
        reason: reason.to_owned(),
    }
}

/// RFC 3339 for payload timestamps.
pub fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn payload(state: &AppState, job: &Job) -> Result<Payload, String> {
    let event: Event = job.event_type.parse()?;
    let severity = job.severity.parse()?;
    let data = &job.event_data.0;
    let message = data
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let state_at_event = if event == Event::Resolved {
        "resolved"
    } else {
        "firing"
    };
    let resource = match (&job.resource_kind, job.hetzner_id, &job.resource_name) {
        (Some(kind), Some(hetzner_id), Some(name)) => Some(ResourceInfo {
            kind: kind.clone(),
            hetzner_id,
            name: name.clone(),
        }),
        _ => None,
    };
    Ok(Payload {
        version: PAYLOAD_VERSION,
        event,
        at: rfc3339(job.event_at),
        alert: AlertInfo {
            id: job.alert_id.to_string(),
            state: state_at_event.to_owned(),
            severity,
            check_type: job.check_type.clone(),
            message,
            value: data.get("value").and_then(serde_json::Value::as_f64),
            fired_at: job.fired_at.map(rfc3339),
            resolved_at: if event == Event::Resolved {
                job.resolved_at.map(rfc3339)
            } else {
                None
            },
        },
        rule: Named {
            id: job.rule_id.to_string(),
            name: job.rule_name.clone(),
        },
        project: Named {
            id: job.project_id.to_string(),
            name: job.project_name.clone(),
        },
        resource,
        link: state
            .base_url
            .as_ref()
            .map(|base| format!("{base}/api/v1/alerts/{}", job.alert_id)),
    })
}
