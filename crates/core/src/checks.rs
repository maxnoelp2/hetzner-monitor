//! Check implementations (docs/design/checks.md). Pure: a check reads a Hetzner resource
//! snapshot (the JSON object as returned by the API) and its rule params, and returns an
//! [`Outcome`]. Field shapes follow the Hetzner `OpenAPI` spec (verified 2026-11-11).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime};

use crate::series;

/// Upper bound for durations in params (one day).
const MAX_DURATION_S: u64 = 86_400;

/// The known checks. Stored as `rules.check_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckType {
    /// Server status ≠ `running`.
    ServerStatus,
    /// Resource created or deleted (one-shot event from the inventory diff).
    ResourceLifecycle,
    /// A load balancer target service is not healthy.
    LbTargetHealth,
    /// Outgoing traffic ≥ threshold of the included traffic.
    TrafficQuota,
    /// The project's Hetzner API access is degraded or the token is invalid (system alert).
    HetznerApi,
    /// CPU usage (normalised by vCPUs) above a threshold for a duration — metrics tier.
    Cpu,
    /// Public network bandwidth or packets above a threshold for a duration — metrics tier.
    Network,
    /// Disk IOPS or bandwidth above a threshold for a duration — metrics tier.
    DiskIo,
    /// Certificate expires soon, or its issuance/renewal failed — inventory tier.
    CertificateExpiry,
    /// Server type (per location) or load balancer type is deprecated — inventory tier.
    Deprecation,
    /// Server without backups — inventory tier.
    HygieneBackups,
    /// Delete (or rebuild) protection off — inventory tier.
    HygieneProtection,
    /// Volume or IP not attached to anything — inventory tier.
    HygieneOrphans,
    /// Estimated monthly cost rose against an earlier day (project-level) — inventory tier.
    CostChange,
}

impl CheckType {
    pub const ALL: [Self; 14] = [
        Self::ServerStatus,
        Self::ResourceLifecycle,
        Self::LbTargetHealth,
        Self::TrafficQuota,
        Self::HetznerApi,
        Self::Cpu,
        Self::Network,
        Self::DiskIo,
        Self::CertificateExpiry,
        Self::Deprecation,
        Self::HygieneBackups,
        Self::HygieneProtection,
        Self::HygieneOrphans,
        Self::CostChange,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ServerStatus => "server_status",
            Self::ResourceLifecycle => "resource_lifecycle",
            Self::LbTargetHealth => "lb_target_health",
            Self::TrafficQuota => "traffic_quota",
            Self::HetznerApi => "hetzner_api",
            Self::Cpu => "cpu",
            Self::Network => "network",
            Self::DiskIo => "disk_io",
            Self::CertificateExpiry => "certificate_expiry",
            Self::Deprecation => "deprecation",
            Self::HygieneBackups => "hygiene_backups",
            Self::HygieneProtection => "hygiene_protection",
            Self::HygieneOrphans => "hygiene_orphans",
            Self::CostChange => "cost_change",
        }
    }

    /// True for checks evaluated on inventory cycles only (they need all six kinds, the
    /// pricing or stored daily samples). Fast cycles leave their alerts untouched.
    /// `traffic_quota` belongs here only with `forecast: true`.
    pub fn inventory_only(self, params: &Value) -> bool {
        match self {
            Self::CertificateExpiry
            | Self::Deprecation
            | Self::HygieneBackups
            | Self::HygieneProtection
            | Self::HygieneOrphans
            | Self::CostChange => true,
            Self::TrafficQuota => parse::<TrafficQuotaParams>(params).is_ok_and(|p| p.forecast),
            _ => false,
        }
    }

    /// The Hetzner metric family (`type` of `GET /servers/{id}/metrics`) a metric check needs;
    /// `None` for checks evaluated on inventory snapshots.
    pub const fn metric_type(self) -> Option<&'static str> {
        match self {
            Self::Cpu => Some("cpu"),
            Self::Network => Some("network"),
            Self::DiskIo => Some("disk"),
            _ => None,
        }
    }

    /// Duration a metric rule needs in its query window (`for_s`); 0 for other checks or invalid params.
    pub fn window_s(self, params: &Value) -> u64 {
        match self {
            Self::Cpu => parse::<CpuParams>(params).map_or(0, |p| p.for_s),
            Self::Network => parse::<RateParams<NetworkMetric>>(params).map_or(0, |p| p.for_s),
            Self::DiskIo => parse::<RateParams<DiskMetric>>(params).map_or(0, |p| p.for_s),
            _ => 0,
        }
    }

    /// Resource kinds (`resources.kind`) this check evaluates per resource. Empty for
    /// project-level checks; `resource_lifecycle` applies to every kind.
    pub const fn kinds(self) -> &'static [&'static str] {
        match self {
            Self::ServerStatus
            | Self::Cpu
            | Self::Network
            | Self::DiskIo
            | Self::HygieneBackups => &["server"],
            Self::LbTargetHealth => &["load_balancer"],
            Self::TrafficQuota | Self::Deprecation => &["server", "load_balancer"],
            Self::CertificateExpiry => &["certificate"],
            Self::HygieneProtection => &PROTECTABLE,
            Self::HygieneOrphans => &["volume", "floating_ip", "primary_ip"],
            Self::ResourceLifecycle | Self::HetznerApi | Self::CostChange => &[],
        }
    }

    /// Checks that `params` are valid for this check (unknown fields are rejected).
    pub fn validate_params(self, params: &Value) -> Result<(), ParamsError> {
        if !params.is_object() {
            return Err(ParamsError("params must be a JSON object".to_owned()));
        }
        match self {
            Self::ServerStatus => parse::<ServerStatusParams>(params)?.validate(),
            Self::LbTargetHealth => parse::<LbTargetHealthParams>(params)?.validate(),
            Self::TrafficQuota => parse::<TrafficQuotaParams>(params)?.validate(),
            Self::ResourceLifecycle | Self::HetznerApi => parse::<NoParams>(params).map(|_| ()),
            Self::Cpu => parse::<CpuParams>(params)?.validate(),
            Self::Network => parse::<RateParams<NetworkMetric>>(params)?.validate(),
            Self::DiskIo => parse::<RateParams<DiskMetric>>(params)?.validate(),
            Self::CertificateExpiry => parse::<CertificateParams>(params)?.validate(),
            Self::Deprecation => parse::<NoParams>(params).map(|_| ()),
            Self::HygieneBackups => parse::<HygieneParams>(params)?.validate(),
            Self::HygieneProtection => parse::<ProtectionParams>(params)?.validate(),
            Self::HygieneOrphans => parse::<OrphanParams>(params)?.validate(),
            Self::CostChange => parse::<CostChangeParams>(params)?.validate(),
        }
    }

    /// Evaluates a per-resource check on `snapshot`. `None` for checks that are not
    /// evaluated per resource (`resource_lifecycle`, `hetzner_api`, metric checks,
    /// `cost_change`) or that do not apply to this resource (e.g. a kind excluded by params).
    pub fn evaluate(
        self,
        params: &Value,
        snapshot: &Value,
        ctx: &Context<'_>,
    ) -> Result<Option<Outcome>, ParamsError> {
        Ok(match self {
            Self::ServerStatus => Some(server_status(snapshot, &parse(params)?)),
            Self::LbTargetHealth => Some(lb_target_health(snapshot, &parse(params)?)),
            Self::TrafficQuota => Some(traffic_quota(snapshot, &parse(params)?, ctx)),
            Self::CertificateExpiry => Some(certificate_expiry(snapshot, &parse(params)?, ctx.now)),
            Self::Deprecation => {
                parse::<NoParams>(params)?;
                Some(deprecation(ctx.kind, snapshot, ctx.now))
            }
            Self::HygieneBackups => Some(hygiene_backups(snapshot, &parse(params)?)),
            Self::HygieneProtection => hygiene_protection(ctx.kind, snapshot, &parse(params)?),
            Self::HygieneOrphans => hygiene_orphans(ctx.kind, snapshot, &parse(params)?),
            Self::ResourceLifecycle
            | Self::HetznerApi
            | Self::Cpu
            | Self::Network
            | Self::DiskIo
            | Self::CostChange => None,
        })
    }

    /// Evaluates the project-level `cost_change` check on today's estimate and the latest
    /// estimate of an earlier day. `None` for other checks.
    pub fn evaluate_cost(
        self,
        params: &Value,
        current: &CostPoint,
        previous: Option<&CostPoint>,
    ) -> Result<Option<Outcome>, ParamsError> {
        Ok(match self {
            Self::CostChange => Some(cost_change(&parse(params)?, current, previous)),
            _ => None,
        })
    }

    /// Evaluates a metric check for one server on the series Hetzner returned (`step` = the
    /// resolution it used, `now` = UNIX seconds). `None` for non-metric checks.
    pub fn evaluate_metrics(
        self,
        params: &Value,
        snapshot: &Value,
        metrics: &BTreeMap<String, Vec<(f64, f64)>>,
        step: f64,
        now: f64,
    ) -> Result<Option<Outcome>, ParamsError> {
        Ok(match self {
            Self::Cpu => Some(cpu(snapshot, &parse(params)?, metrics, step, now)),
            Self::Network => Some(rate(
                snapshot,
                &parse::<RateParams<NetworkMetric>>(params)?,
                metrics,
                step,
                now,
            )),
            Self::DiskIo => Some(rate(
                snapshot,
                &parse::<RateParams<DiskMetric>>(params)?,
                metrics,
                step,
                now,
            )),
            _ => None,
        })
    }
}

impl FromStr for CheckType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown check type `{s}`"))
    }
}

impl fmt::Display for CheckType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Kinds with a `protection.delete` flag (servers also have `protection.rebuild`).
const PROTECTABLE: [&str; 5] = [
    "server",
    "load_balancer",
    "volume",
    "floating_ip",
    "primary_ip",
];

/// What a per-resource check may need besides the snapshot. Pure data, built by the engine.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// Evaluation time, UNIX seconds.
    pub now: i64,
    /// The resource's kind (`server`, `volume`, …).
    pub kind: &'a str,
    /// Forecast baseline: the newest stored daily traffic sample at least a day old.
    pub traffic_baseline: Option<TrafficSample>,
}

/// An outgoing-traffic counter reading (`traffic_samples`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficSample {
    /// UNIX seconds.
    pub at: i64,
    /// Bytes, as reported in `outgoing_traffic`.
    pub outgoing: u64,
}

/// A stored daily cost estimate (`cost_estimates`).
#[derive(Debug, Clone, PartialEq)]
pub struct CostPoint {
    /// UTC day, `YYYY-MM-DD`.
    pub day: String,
    pub monthly_net: f64,
    pub currency: String,
}

/// Result of a check for one resource.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// True if the alert condition holds.
    pub condition: bool,
    /// Measured value, if the check has one (e.g. traffic percentage).
    pub value: Option<f64>,
    /// Human-readable description of the current state.
    pub message: String,
    /// How long the condition must hold before firing (seconds).
    pub hold_s: u64,
    /// How long the condition has held, measured by the check (metric series). `None` = use
    /// the alert's local pending time.
    pub held_s: Option<u64>,
}

/// Invalid rule params.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid params: {0}")]
pub struct ParamsError(String);

fn parse<T: DeserializeOwned>(params: &Value) -> Result<T, ParamsError> {
    serde_json::from_value(params.clone()).map_err(|e| ParamsError(e.to_string()))
}

fn check_duration(name: &str, value: u64) -> Result<(), ParamsError> {
    if value > MAX_DURATION_S {
        return Err(ParamsError(format!("`{name}` must be ≤ {MAX_DURATION_S}")));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

const fn default_for_s() -> u64 {
    120
}

const fn default_grace_s() -> u64 {
    600
}

const fn default_threshold_pct() -> f64 {
    80.0
}

/// Params of `server_status`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerStatusParams {
    /// Seconds a server must be not `running` before firing.
    #[serde(default = "default_for_s")]
    pub for_s: u64,
    /// Seconds a transitional status (starting, migrating, …) is tolerated.
    #[serde(default = "default_grace_s")]
    pub transitional_grace_s: u64,
}

impl ServerStatusParams {
    fn validate(&self) -> Result<(), ParamsError> {
        check_duration("for_s", self.for_s)?;
        check_duration("transitional_grace_s", self.transitional_grace_s)
    }
}

/// Server statuses that are expected to pass on their own.
const TRANSITIONAL: [&str; 5] = [
    "initializing",
    "starting",
    "migrating",
    "rebuilding",
    "deleting",
];

/// `server_status`: fires when the server is not `running`. Transitional statuses only
/// fire after `transitional_grace_s`.
pub fn server_status(snapshot: &Value, params: &ServerStatusParams) -> Outcome {
    let name = display_name(snapshot);
    let status = snapshot
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if status == "running" {
        return Outcome {
            condition: false,
            value: None,
            message: format!("server {name} is running"),
            hold_s: params.for_s,
            held_s: None,
        };
    }
    let hold_s = if TRANSITIONAL.contains(&status) {
        params.transitional_grace_s.max(params.for_s)
    } else {
        params.for_s
    };
    Outcome {
        condition: true,
        value: None,
        message: format!("server {name} is {status}"),
        hold_s,
        held_s: None,
    }
}

/// Params of `lb_target_health`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LbTargetHealthParams {
    /// Seconds a target must be unhealthy before firing.
    #[serde(default = "default_for_s")]
    pub for_s: u64,
}

impl LbTargetHealthParams {
    fn validate(&self) -> Result<(), ParamsError> {
        check_duration("for_s", self.for_s)
    }
}

/// `lb_target_health`: fires when any target service reports a status other than `healthy`.
/// Label-selector targets carry their resolved servers in a nested `targets` list.
pub fn lb_target_health(snapshot: &Value, params: &LbTargetHealthParams) -> Outcome {
    let name = display_name(snapshot);
    let mut total = 0_u32;
    let mut bad = Vec::new();
    let empty = Vec::new();
    let targets = snapshot
        .get("targets")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let mut visit = |target: &Value| {
        let label = target_label(target);
        for service in target
            .get("health_status")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            total += 1;
            let status = service
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if status != "healthy" {
                let port = service
                    .get("listen_port")
                    .and_then(Value::as_i64)
                    .map_or_else(|| "?".to_owned(), |p| p.to_string());
                bad.push(format!("{label}:{port} {status}"));
            }
        }
    };
    for target in targets {
        visit(target);
        for nested in target
            .get("targets")
            .and_then(Value::as_array)
            .unwrap_or(&empty)
        {
            visit(nested);
        }
    }
    let message = if bad.is_empty() {
        format!("load balancer {name}: all {total} target services healthy")
    } else {
        format!(
            "load balancer {name}: {} of {total} target services not healthy ({})",
            bad.len(),
            bad.join(", ")
        )
    };
    Outcome {
        condition: !bad.is_empty(),
        value: Some(f64::from(u32::try_from(bad.len()).unwrap_or(u32::MAX))),
        message,
        hold_s: params.for_s,
        held_s: None,
    }
}

fn target_label(target: &Value) -> String {
    if let Some(id) = target.pointer("/server/id").and_then(Value::as_i64) {
        return format!("server {id}");
    }
    if let Some(ip) = target.pointer("/ip/ip").and_then(Value::as_str) {
        return format!("ip {ip}");
    }
    "target".to_owned()
}

/// Params of `traffic_quota`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrafficQuotaParams {
    /// Percentage of the included traffic that triggers the alert.
    #[serde(default = "default_threshold_pct")]
    pub threshold_pct: f64,
    /// Seconds the threshold must be exceeded before firing.
    #[serde(default)]
    pub for_s: u64,
    /// Compare the forecast for the end of the calendar month instead of the current use.
    #[serde(default)]
    pub forecast: bool,
}

impl TrafficQuotaParams {
    fn validate(&self) -> Result<(), ParamsError> {
        if !(self.threshold_pct > 0.0 && self.threshold_pct <= 1000.0) {
            return Err(ParamsError(
                "`threshold_pct` must be > 0 and ≤ 1000".to_owned(),
            ));
        }
        check_duration("for_s", self.for_s)
    }
}

/// `traffic_quota`: `outgoing_traffic / included_traffic` (current billing period) ≥ threshold,
/// or with `forecast` the extrapolated use at the end of the calendar month.
/// Without both numbers (Hetzner returns `null` until there is data) the condition is false.
pub fn traffic_quota(snapshot: &Value, params: &TrafficQuotaParams, ctx: &Context<'_>) -> Outcome {
    let name = display_name(snapshot);
    let get = |field: &str| snapshot.get(field).and_then(Value::as_u64);
    let (Some(outgoing), Some(included)) = (get("outgoing_traffic"), get("included_traffic"))
    else {
        return Outcome {
            condition: false,
            value: None,
            message: format!("{name}: no traffic data yet"),
            hold_s: params.for_s,
            held_s: None,
        };
    };
    if included == 0 {
        return Outcome {
            condition: false,
            value: None,
            message: format!("{name}: no included traffic"),
            hold_s: params.for_s,
            held_s: None,
        };
    }
    if params.forecast {
        return traffic_forecast(&name, outgoing, included, params, ctx);
    }
    // Byte counts far below 2^52, so the conversion is exact enough for a percentage.
    #[allow(clippy::cast_precision_loss)]
    let pct = outgoing as f64 / included as f64 * 100.0;
    let pct = (pct * 10.0).round() / 10.0;
    Outcome {
        condition: pct >= params.threshold_pct,
        value: Some(pct),
        message: format!(
            "{name}: {pct} % of the included traffic used ({} of {} GB)",
            gb(outgoing),
            gb(included)
        ),
        hold_s: params.for_s,
        held_s: None,
    }
}

/// Minimum span between the baseline sample and now for a forecast.
pub const FORECAST_MIN_SPAN_S: i64 = 86_400;

/// Linear forecast to the end of the calendar month (UTC) from the baseline sample. The billing
/// period is not exposed by Hetzner, so this is a labelled estimate (docs/design/checks.md).
fn traffic_forecast(
    name: &str,
    outgoing: u64,
    included: u64,
    params: &TrafficQuotaParams,
    ctx: &Context<'_>,
) -> Outcome {
    let no_data = |message: String| Outcome {
        condition: false,
        value: None,
        message,
        hold_s: params.for_s,
        held_s: None,
    };
    let Some(base) = ctx
        .traffic_baseline
        .filter(|b| ctx.now - b.at >= FORECAST_MIN_SPAN_S)
    else {
        return no_data(format!(
            "{name}: no traffic forecast yet (needs a sample from at least a day ago)"
        ));
    };
    if outgoing < base.outgoing {
        return no_data(format!(
            "{name}: traffic counter was reset since {}; forecast resumes in a day",
            date(base.at)
        ));
    }
    let end = next_month_start(ctx.now);
    // Spans and byte counts are far below 2^52.
    #[allow(clippy::cast_precision_loss)]
    let forecast = {
        let rate = (outgoing - base.outgoing) as f64 / (ctx.now - base.at) as f64;
        outgoing as f64 + rate * (end - ctx.now) as f64
    };
    #[allow(clippy::cast_precision_loss)]
    let pct = round1(forecast / included as f64 * 100.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let forecast_bytes = forecast.max(0.0) as u64;
    Outcome {
        condition: pct >= params.threshold_pct,
        value: Some(pct),
        message: format!(
            "{name}: forecast {pct} % of the included traffic by {} ({} of {} GB; estimate, assumes the quota resets on the 1st, UTC)",
            date(end - 1),
            gb(forecast_bytes),
            gb(included)
        ),
        hold_s: params.for_s,
        held_s: None,
    }
}

/// UNIX seconds of the first day of the month after `now`, 00:00 UTC.
fn next_month_start(now: i64) -> i64 {
    let day = OffsetDateTime::from_unix_timestamp(now)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .date();
    let (year, month) = if day.month() == Month::December {
        (day.year() + 1, Month::January)
    } else {
        (day.year(), day.month().next())
    };
    Date::from_calendar_date(year, month, 1)
        .map_or(now, |d| d.midnight().assume_utc().unix_timestamp())
}

/// UTC date of a UNIX timestamp, `YYYY-MM-DD`.
fn date(ts: i64) -> String {
    OffsetDateTime::from_unix_timestamp(ts)
        .map(|t| t.date().to_string())
        .unwrap_or_default()
}

/// RFC 3339 timestamp field → UNIX seconds.
fn timestamp(value: Option<&Value>) -> Option<i64> {
    let text = value?.as_str()?;
    OffsetDateTime::parse(text, &Rfc3339)
        .ok()
        .map(OffsetDateTime::unix_timestamp)
}

#[allow(clippy::cast_precision_loss)]
fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / 1e9)
}

const fn default_cpu_threshold() -> f64 {
    90.0
}

const fn default_cpu_for_s() -> u64 {
    600
}

/// Params of `cpu`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CpuParams {
    /// Percent of the server's total CPU (all vCPUs = 100 %).
    #[serde(default = "default_cpu_threshold")]
    pub threshold_pct: f64,
    /// Seconds the threshold must be exceeded, measured on Hetzner's series.
    #[serde(default = "default_cpu_for_s")]
    pub for_s: u64,
}

impl CpuParams {
    fn validate(&self) -> Result<(), ParamsError> {
        if !(self.threshold_pct > 0.0 && self.threshold_pct <= 100.0) {
            return Err(ParamsError(
                "`threshold_pct` must be > 0 and ≤ 100".to_owned(),
            ));
        }
        check_duration("for_s", self.for_s)
    }
}

/// `cpu`: Hetzner reports percent **per vCPU** (100 = one full core, spec 2026-11-11), so the
/// value is divided by `server_type.cores` from the server snapshot.
pub fn cpu(
    snapshot: &Value,
    params: &CpuParams,
    metrics: &BTreeMap<String, Vec<(f64, f64)>>,
    step: f64,
    now: f64,
) -> Outcome {
    let name = display_name(snapshot);
    let cores = snapshot
        .pointer("/server_type/cores")
        .and_then(Value::as_f64)
        .filter(|c| *c >= 1.0)
        .unwrap_or(1.0);
    let samples = metrics.get("cpu").map_or(&[][..], Vec::as_slice);
    let trailing = series::trailing(samples, step, now, |v| v / cores > params.threshold_pct);
    let Some(t) = trailing else {
        return no_data(&name, "CPU", params.for_s);
    };
    let pct = round1(t.latest / cores);
    let message = if t.breaching {
        format!(
            "server {name}: CPU {pct} % of {cores} vCPUs for {} (threshold {} % for {})",
            duration(t.held_s),
            params.threshold_pct,
            duration(params.for_s)
        )
    } else {
        format!("server {name}: CPU {pct} % of {cores} vCPUs")
    };
    Outcome {
        condition: t.breaching,
        value: Some(pct),
        message,
        hold_s: params.for_s,
        held_s: Some(t.held_s),
    }
}

/// A rate metric of the `network` or `disk_io` check.
pub trait RateMetric: Copy + fmt::Debug {
    /// Hetzner series name.
    fn series(self) -> &'static str;
    /// Unit for messages.
    fn unit(self) -> &'static str;
    /// Human label.
    fn label(self) -> &'static str;
}

/// `network` metrics (public interface only).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMetric {
    BandwidthIn,
    BandwidthOut,
    PpsIn,
    PpsOut,
}

impl RateMetric for NetworkMetric {
    fn series(self) -> &'static str {
        match self {
            Self::BandwidthIn => "network.0.bandwidth.in",
            Self::BandwidthOut => "network.0.bandwidth.out",
            Self::PpsIn => "network.0.pps.in",
            Self::PpsOut => "network.0.pps.out",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::BandwidthIn | Self::BandwidthOut => "B/s",
            Self::PpsIn | Self::PpsOut => "packets/s",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::BandwidthIn => "network in",
            Self::BandwidthOut => "network out",
            Self::PpsIn => "packets in",
            Self::PpsOut => "packets out",
        }
    }
}

/// `disk_io` metrics.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiskMetric {
    IopsRead,
    IopsWrite,
    BandwidthRead,
    BandwidthWrite,
}

impl RateMetric for DiskMetric {
    fn series(self) -> &'static str {
        match self {
            Self::IopsRead => "disk.0.iops.read",
            Self::IopsWrite => "disk.0.iops.write",
            Self::BandwidthRead => "disk.0.bandwidth.read",
            Self::BandwidthWrite => "disk.0.bandwidth.write",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::IopsRead | Self::IopsWrite => "IOPS",
            Self::BandwidthRead | Self::BandwidthWrite => "B/s",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::IopsRead => "disk reads",
            Self::IopsWrite => "disk writes",
            Self::BandwidthRead => "disk read bandwidth",
            Self::BandwidthWrite => "disk write bandwidth",
        }
    }
}

const fn default_rate_for_s() -> u64 {
    300
}

/// Params of `network` and `disk_io`: templates without a default threshold.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateParams<M> {
    pub metric: M,
    /// In the metric's unit (bytes/s, packets/s or IOPS). Required.
    pub threshold: f64,
    #[serde(default = "default_rate_for_s")]
    pub for_s: u64,
}

impl<M> RateParams<M> {
    fn validate(&self) -> Result<(), ParamsError> {
        if !(self.threshold.is_finite() && self.threshold > 0.0) {
            return Err(ParamsError("`threshold` must be > 0".to_owned()));
        }
        check_duration("for_s", self.for_s)
    }
}

/// `network` / `disk_io`: the chosen series above `threshold` for `for_s`.
pub fn rate<M: RateMetric>(
    snapshot: &Value,
    params: &RateParams<M>,
    metrics: &BTreeMap<String, Vec<(f64, f64)>>,
    step: f64,
    now: f64,
) -> Outcome {
    let name = display_name(snapshot);
    let metric = params.metric;
    let samples = metrics.get(metric.series()).map_or(&[][..], Vec::as_slice);
    let Some(t) = series::trailing(samples, step, now, |v| v > params.threshold) else {
        return no_data(&name, metric.label(), params.for_s);
    };
    let value = round1(t.latest);
    let message = if t.breaching {
        format!(
            "server {name}: {} {value} {} for {} (threshold {} for {})",
            metric.label(),
            metric.unit(),
            duration(t.held_s),
            params.threshold,
            duration(params.for_s)
        )
    } else {
        format!(
            "server {name}: {} {value} {}",
            metric.label(),
            metric.unit()
        )
    };
    Outcome {
        condition: t.breaching,
        value: Some(value),
        message,
        hold_s: params.for_s,
        held_s: Some(t.held_s),
    }
}

fn no_data(name: &str, what: &str, hold_s: u64) -> Outcome {
    Outcome {
        condition: false,
        value: None,
        message: format!("server {name}: no recent {what} data"),
        hold_s,
        held_s: None,
    }
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// `600` → `10 min`, `45` → `45 s`, `3900` → `1 h 5 min`.
fn duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s} s"),
        s if s < 3600 => format!("{} min", s / 60),
        s if s % 3600 / 60 == 0 => format!("{} h", s / 3600),
        s => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

const fn default_cert_days() -> u32 {
    14
}

const fn default_true() -> bool {
    true
}

/// Params of `certificate_expiry`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CertificateParams {
    /// Fire when the certificate expires in fewer than this many days.
    #[serde(default = "default_cert_days")]
    pub days: u32,
    /// Also fire when Hetzner reports a failed issuance or renewal.
    #[serde(default = "default_true")]
    pub on_failure: bool,
}

impl CertificateParams {
    fn validate(&self) -> Result<(), ParamsError> {
        if !(1..=365).contains(&self.days) {
            return Err(ParamsError("`days` must be between 1 and 365".to_owned()));
        }
        Ok(())
    }
}

/// `certificate_expiry`: `not_valid_after` closer than `days`, or (with `on_failure`)
/// `status.issuance` / `status.renewal` = `failed`. Uploaded certificates have no `status`.
/// Value = whole days left (negative once expired).
pub fn certificate_expiry(snapshot: &Value, params: &CertificateParams, now: i64) -> Outcome {
    let name = format!("certificate {}", display_name(snapshot));
    let failure = params
        .on_failure
        .then(|| {
            ["issuance", "renewal"].into_iter().find(|step| {
                snapshot
                    .pointer(&format!("/status/{step}"))
                    .and_then(Value::as_str)
                    == Some("failed")
            })
        })
        .flatten()
        .map(|step| {
            let reason = snapshot
                .pointer("/status/error/message")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .unwrap_or("no details");
            format!("{step} failed ({reason})")
        });
    let expiry = timestamp(snapshot.get("not_valid_after"));
    let days_left = expiry.map(|e| (e - now).div_euclid(86_400));
    let expiring = expiry.is_some_and(|e| e - now < i64::from(params.days) * 86_400);
    let validity = match (expiry, days_left) {
        (Some(e), _) if e <= now => format!("expired on {}", date(e)),
        (Some(e), Some(d)) => format!("expires in {d} days ({})", date(e)),
        _ => "no expiry date yet".to_owned(),
    };
    let message = match &failure {
        Some(f) => format!("{name}: {f}; {validity}"),
        None if expiring => format!("{name}: {validity}"),
        None => match (expiry, days_left) {
            (Some(e), Some(d)) => format!("{name}: valid until {} ({d} days)", date(e)),
            _ => format!("{name}: {validity}"),
        },
    };
    Outcome {
        condition: failure.is_some() || expiring,
        #[allow(clippy::cast_precision_loss)]
        value: days_left.map(|d| d as f64),
        message,
        hold_s: 0,
        held_s: None,
    }
}

/// `deprecation`: a server's type is deprecated **in its location**
/// (`server_type.locations[].deprecation`; the deprecated top-level `server_type.deprecation`
/// as fallback), or a load balancer's type is deprecated. Value = days until `unavailable_after`.
pub fn deprecation(kind: &str, snapshot: &Value, now: i64) -> Outcome {
    let name = format!("{} {}", kind.replace('_', " "), display_name(snapshot));
    let location = snapshot
        .pointer("/location/name")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let not_null = |v: Option<&Value>| v.filter(|d| d.is_object()).cloned();
    let (type_label, type_obj, where_) = if kind == "server" {
        (
            "server type",
            snapshot.get("server_type"),
            format!(" in {location}"),
        )
    } else {
        (
            "load balancer type",
            snapshot.get("load_balancer_type"),
            String::new(),
        )
    };
    let type_name = type_obj
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let per_location = (kind == "server")
        .then(|| {
            type_obj
                .and_then(|t| t.get("locations"))
                .and_then(Value::as_array)
                .and_then(|locs| {
                    locs.iter()
                        .find(|l| l.get("name").and_then(Value::as_str) == Some(location))
                })
                .and_then(|l| not_null(l.get("deprecation")))
        })
        .flatten();
    let dep = per_location.or_else(|| not_null(type_obj.and_then(|t| t.get("deprecation"))));
    let Some(dep) = dep else {
        return Outcome {
            condition: false,
            value: None,
            message: format!("{name}: {type_label} {type_name}{where_} is not deprecated"),
            hold_s: 0,
            held_s: None,
        };
    };
    let unavailable = timestamp(dep.get("unavailable_after"));
    let announced = timestamp(dep.get("announced"))
        .map(|a| format!(" (announced {})", date(a)))
        .unwrap_or_default();
    let after = unavailable.map_or_else(
        || "no date given".to_owned(),
        |u| format!("unavailable after {}", date(u)),
    );
    Outcome {
        condition: true,
        #[allow(clippy::cast_precision_loss)]
        value: unavailable.map(|u| (u - now).div_euclid(86_400) as f64),
        message: format!(
            "{name}: {type_label} {type_name} is deprecated{where_}, {after}{announced}"
        ),
        hold_s: 0,
        held_s: None,
    }
}

const fn default_hygiene_for_s() -> u64 {
    3600
}

const fn default_orphan_for_s() -> u64 {
    86_400
}

/// Params of `hygiene_backups`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HygieneParams {
    /// Grace period for new servers (seconds).
    #[serde(default = "default_hygiene_for_s")]
    pub for_s: u64,
}

impl HygieneParams {
    fn validate(&self) -> Result<(), ParamsError> {
        check_duration("for_s", self.for_s)
    }
}

/// `hygiene_backups`: `backup_window` null means backups are off.
pub fn hygiene_backups(snapshot: &Value, params: &HygieneParams) -> Outcome {
    let name = display_name(snapshot);
    let window = snapshot.get("backup_window").and_then(Value::as_str);
    Outcome {
        condition: window.is_none(),
        value: None,
        message: window.map_or_else(
            || format!("server {name}: backups disabled"),
            |w| format!("server {name}: backups enabled (window {w} UTC)"),
        ),
        hold_s: params.for_s,
        held_s: None,
    }
}

fn default_protection_kinds() -> Vec<String> {
    vec!["server".to_owned()]
}

/// Params of `hygiene_protection`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtectionParams {
    /// Kinds to check (subset of server, load balancer, volume, floating IP, primary IP).
    #[serde(default = "default_protection_kinds")]
    pub kinds: Vec<String>,
    #[serde(default = "default_hygiene_for_s")]
    pub for_s: u64,
}

impl ProtectionParams {
    fn validate(&self) -> Result<(), ParamsError> {
        if self.kinds.is_empty() {
            return Err(ParamsError("`kinds` must not be empty".to_owned()));
        }
        if let Some(bad) = self
            .kinds
            .iter()
            .find(|k| !PROTECTABLE.contains(&k.as_str()))
        {
            return Err(ParamsError(format!(
                "`kinds`: `{bad}` has no protection (allowed: {})",
                PROTECTABLE.join(", ")
            )));
        }
        check_duration("for_s", self.for_s)
    }
}

/// `hygiene_protection`: `protection.delete` off (servers: or `protection.rebuild` off).
/// `None` for kinds not listed in `params.kinds`.
pub fn hygiene_protection(
    kind: &str,
    snapshot: &Value,
    params: &ProtectionParams,
) -> Option<Outcome> {
    if !params.kinds.iter().any(|k| k == kind) {
        return None;
    }
    let name = format!("{} {}", kind.replace('_', " "), display_name(snapshot));
    let flag = |f: &str| {
        snapshot
            .pointer(&format!("/protection/{f}"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let mut off = Vec::new();
    if !flag("delete") {
        off.push("delete");
    }
    if kind == "server" && !flag("rebuild") {
        off.push("rebuild");
    }
    Some(Outcome {
        condition: !off.is_empty(),
        value: None,
        message: if off.is_empty() {
            format!("{name}: protection on")
        } else {
            format!("{name}: {} protection off", off.join(" and "))
        },
        hold_s: params.for_s,
        held_s: None,
    })
}

/// Params of `hygiene_orphans`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrphanParams {
    /// How long a resource may stay unattached before it counts (seconds).
    #[serde(default = "default_orphan_for_s")]
    pub for_s: u64,
}

impl OrphanParams {
    fn validate(&self) -> Result<(), ParamsError> {
        check_duration("for_s", self.for_s)
    }
}

/// `hygiene_orphans`: a volume without `server` (not while `creating`), a floating IP without
/// `server`, a primary IP without `assignee_id` — they cost money and do nothing.
pub fn hygiene_orphans(kind: &str, snapshot: &Value, params: &OrphanParams) -> Option<Outcome> {
    let mut name = format!("{} {}", kind.replace('_', " "), display_name(snapshot));
    if let Some(ip) = snapshot.get("ip").and_then(Value::as_str) {
        name = format!("{name} [{ip}]");
    }
    let unset = |field: &str| snapshot.get(field).is_none_or(Value::is_null);
    let (orphan, what) = match kind {
        "volume" => {
            let creating = snapshot.get("status").and_then(Value::as_str) == Some("creating");
            (!creating && unset("server"), "not attached to a server")
        }
        "floating_ip" => (unset("server"), "not assigned to a server"),
        "primary_ip" => (unset("assignee_id"), "not assigned to a server"),
        _ => return None,
    };
    Some(Outcome {
        condition: orphan,
        value: None,
        message: if orphan {
            format!("{name}: {what}")
        } else {
            format!("{name}: in use")
        },
        hold_s: params.for_s,
        held_s: None,
    })
}

const fn default_cost_threshold() -> f64 {
    10.0
}

/// Params of `cost_change`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostChangeParams {
    /// Increase in percent against the latest estimate of an earlier day.
    #[serde(default = "default_cost_threshold")]
    pub threshold_pct: f64,
}

impl CostChangeParams {
    fn validate(&self) -> Result<(), ParamsError> {
        if !(self.threshold_pct > 0.0 && self.threshold_pct <= 1000.0) {
            return Err(ParamsError(
                "`threshold_pct` must be > 0 and ≤ 1000".to_owned(),
            ));
        }
        Ok(())
    }
}

/// `cost_change`: today's estimated monthly cost is at least `threshold_pct` above the latest
/// estimate of an earlier day. Value = change in percent.
pub fn cost_change(
    params: &CostChangeParams,
    current: &CostPoint,
    previous: Option<&CostPoint>,
) -> Outcome {
    let now = format!("{:.2} {}", current.monthly_net, current.currency);
    let no_data = |message: String| Outcome {
        condition: false,
        value: None,
        message,
        hold_s: 0,
        held_s: None,
    };
    let Some(prev) = previous else {
        return no_data(format!(
            "estimated monthly cost {now} (no earlier day to compare yet)"
        ));
    };
    if prev.currency != current.currency {
        return no_data(format!(
            "estimated monthly cost {now} (currency changed since {})",
            prev.day
        ));
    }
    let before = format!("{:.2} {}", prev.monthly_net, prev.currency);
    let (pct, condition) = if prev.monthly_net > 0.0 {
        let pct = round1((current.monthly_net - prev.monthly_net) / prev.monthly_net * 100.0);
        (Some(pct), pct >= params.threshold_pct)
    } else {
        (None, current.monthly_net > 0.0)
    };
    let change = pct.map_or_else(|| "up from nothing".to_owned(), |p| format!("{p:+} %"));
    Outcome {
        condition,
        value: pct,
        message: format!(
            "estimated monthly cost {now}, {change} vs. {} ({before}); net, without traffic overage and snapshots",
            prev.day
        ),
        hold_s: 0,
        held_s: None,
    }
}

/// Message for a `resource_lifecycle` event.
pub fn lifecycle_message(kind: &str, name: &str, hetzner_id: i64, created: bool) -> String {
    let what = kind.replace('_', " ");
    let label = if name.is_empty() {
        hetzner_id.to_string()
    } else {
        format!("{name} ({hetzner_id})")
    };
    if created {
        format!("{what} {label} created")
    } else {
        format!("{what} {label} deleted")
    }
}

fn display_name(snapshot: &Value) -> String {
    let id = snapshot
        .get("id")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    match snapshot.get("name").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => format!("{name} ({id})"),
        _ => id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn server(status: &str) -> Value {
        json!({ "id": 42, "name": "web-1", "status": status, "labels": {} })
    }

    fn ss_params() -> ServerStatusParams {
        parse(&json!({})).unwrap()
    }

    #[test]
    fn running_server_is_ok() {
        let out = server_status(&server("running"), &ss_params());
        assert!(!out.condition);
    }

    #[test]
    fn off_server_fires_after_for_s() {
        let out = server_status(&server("off"), &ss_params());
        assert!(out.condition);
        assert_eq!(out.hold_s, 120);
        assert_eq!(out.message, "server web-1 (42) is off");
    }

    #[test]
    fn transitional_status_gets_the_grace_period() {
        for status in ["starting", "migrating", "rebuilding", "initializing"] {
            let out = server_status(&server(status), &ss_params());
            assert!(out.condition, "{status}");
            assert_eq!(out.hold_s, 600, "{status}");
        }
        assert_eq!(server_status(&server("stopping"), &ss_params()).hold_s, 120);
        assert_eq!(server_status(&server("unknown"), &ss_params()).hold_s, 120);
    }

    #[test]
    fn missing_status_counts_as_unknown() {
        let out = server_status(&json!({ "id": 1 }), &ss_params());
        assert!(out.condition);
        assert_eq!(out.message, "server 1 is unknown");
    }

    fn lb(targets: &Value) -> Value {
        json!({ "id": 7, "name": "lb-1", "targets": targets })
    }

    #[test]
    fn healthy_lb_is_ok() {
        let snap = lb(&json!([
            { "type": "server", "server": { "id": 1 }, "health_status": [
                { "listen_port": 443, "status": "healthy" }, { "listen_port": 80, "status": "healthy" }
            ]}
        ]));
        let out = lb_target_health(&snap, &parse(&json!({})).unwrap());
        assert!(!out.condition);
        assert_eq!(out.value, Some(0.0));
        assert_eq!(
            out.message,
            "load balancer lb-1 (7): all 2 target services healthy"
        );
    }

    #[test]
    fn unhealthy_and_unknown_targets_fire() {
        let snap = lb(&json!([
            { "type": "server", "server": { "id": 1 }, "health_status": [{ "listen_port": 443, "status": "unhealthy" }] },
            { "type": "ip", "ip": { "ip": "203.0.113.5" }, "health_status": [{ "listen_port": 443, "status": "unknown" }] },
            { "type": "label_selector", "label_selector": { "selector": "role=web" }, "targets": [
                { "type": "server", "server": { "id": 3 }, "health_status": [{ "listen_port": 443, "status": "healthy" }] }
            ]}
        ]));
        let out = lb_target_health(&snap, &parse(&json!({ "for_s": 60 })).unwrap());
        assert!(out.condition);
        assert_eq!(out.value, Some(2.0));
        assert_eq!(out.hold_s, 60);
        assert_eq!(
            out.message,
            "load balancer lb-1 (7): 2 of 3 target services not healthy (server 1:443 unhealthy, ip 203.0.113.5:443 unknown)"
        );
    }

    #[test]
    fn lb_without_targets_is_ok() {
        let out = lb_target_health(&json!({ "id": 7 }), &parse(&json!({})).unwrap());
        assert!(!out.condition);
    }

    fn traffic(out: Option<u64>, included: Option<u64>) -> Value {
        json!({ "id": 42, "name": "web-1", "outgoing_traffic": out, "included_traffic": included })
    }

    const NOW: i64 = 1_794_484_800; // 2026-11-12 12:00 UTC

    fn ctx(kind: &str) -> Context<'_> {
        Context {
            now: NOW,
            kind,
            traffic_baseline: None,
        }
    }

    fn traffic_quota_now(snapshot: &Value, params: &TrafficQuotaParams) -> Outcome {
        traffic_quota(snapshot, params, &ctx("server"))
    }

    #[test]
    fn traffic_below_and_above_threshold() {
        let p: TrafficQuotaParams = parse(&json!({ "threshold_pct": 80 })).unwrap();
        let tb = 1_000_000_000_000_u64;
        let low = traffic_quota_now(&traffic(Some(tb / 2), Some(tb)), &p);
        assert!(!low.condition);
        assert_eq!(low.value, Some(50.0));
        let high = traffic_quota_now(&traffic(Some(tb * 8 / 10), Some(tb)), &p);
        assert!(high.condition);
        assert_eq!(
            high.message,
            "web-1 (42): 80 % of the included traffic used (800.0 of 1000.0 GB)"
        );
        assert_eq!(high.hold_s, 0);
    }

    #[test]
    fn traffic_without_data_is_ok() {
        let p: TrafficQuotaParams = parse(&json!({})).unwrap();
        assert!(!traffic_quota_now(&traffic(None, Some(1)), &p).condition);
        assert!(!traffic_quota_now(&traffic(Some(5), None), &p).condition);
        assert!(!traffic_quota_now(&traffic(Some(5), Some(0)), &p).condition);
    }

    #[test]
    fn params_are_validated() {
        let ok = [
            (CheckType::ServerStatus, json!({})),
            (
                CheckType::ServerStatus,
                json!({ "for_s": 300, "transitional_grace_s": 900 }),
            ),
            (CheckType::TrafficQuota, json!({ "threshold_pct": 95.5 })),
            (CheckType::LbTargetHealth, json!({ "for_s": 0 })),
            (CheckType::HetznerApi, json!({})),
        ];
        for (check, params) in ok {
            assert!(check.validate_params(&params).is_ok(), "{check} {params}");
        }
        let bad = [
            (CheckType::ServerStatus, json!({ "for": 300 })),
            (CheckType::ServerStatus, json!({ "for_s": -1 })),
            (CheckType::ServerStatus, json!({ "for_s": 100_000 })),
            (CheckType::TrafficQuota, json!({ "threshold_pct": 0 })),
            (CheckType::TrafficQuota, json!({ "threshold_pct": "80" })),
            (CheckType::ResourceLifecycle, json!({ "x": 1 })),
            (CheckType::HetznerApi, json!([])),
        ];
        for (check, params) in bad {
            assert!(check.validate_params(&params).is_err(), "{check} {params}");
        }
    }

    #[test]
    fn check_types_round_trip() {
        for check in CheckType::ALL {
            assert_eq!(check.as_str().parse::<CheckType>(), Ok(check));
        }
        assert!("memory".parse::<CheckType>().is_err());
    }

    #[test]
    fn lifecycle_messages() {
        assert_eq!(
            lifecycle_message("load_balancer", "lb-1", 7, true),
            "load balancer lb-1 (7) created"
        );
        assert_eq!(
            lifecycle_message("primary_ip", "", 9, false),
            "primary ip 9 deleted"
        );
    }

    fn cpu_series(values: &[f64]) -> BTreeMap<String, Vec<(f64, f64)>> {
        let now = 1_800_000_000.0;
        let n = values.len();
        let samples = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                (
                    now - 60.0 * f64::from(u32::try_from(n - 1 - i).unwrap()),
                    *v,
                )
            })
            .collect();
        BTreeMap::from([("cpu".to_owned(), samples)])
    }

    fn two_core_server() -> Value {
        json!({ "id": 42, "name": "web-1", "status": "running", "server_type": { "cores": 2 } })
    }

    #[test]
    fn cpu_is_normalised_by_vcpus() {
        // 190 % per-vCPU sum on 2 vCPUs = 95 %, for 10 minutes.
        let metrics = cpu_series(&[190.0; 10]);
        let out = CheckType::Cpu
            .evaluate_metrics(
                &json!({}),
                &two_core_server(),
                &metrics,
                60.0,
                1_800_000_000.0,
            )
            .unwrap()
            .unwrap();
        assert!(out.condition);
        assert_eq!(out.value, Some(95.0));
        assert_eq!(out.hold_s, 600);
        assert_eq!(out.held_s, Some(600));
        assert_eq!(
            out.message,
            "server web-1 (42): CPU 95 % of 2 vCPUs for 10 min (threshold 90 % for 10 min)"
        );
        // 150 on 2 vCPUs = 75 %: fine.
        let ok = CheckType::Cpu
            .evaluate_metrics(
                &json!({}),
                &two_core_server(),
                &cpu_series(&[150.0; 10]),
                60.0,
                1_800_000_000.0,
            )
            .unwrap()
            .unwrap();
        assert!(!ok.condition);
        assert_eq!(ok.value, Some(75.0));
    }

    #[test]
    fn cpu_without_data_never_fires() {
        let out = CheckType::Cpu
            .evaluate_metrics(
                &json!({}),
                &two_core_server(),
                &BTreeMap::new(),
                60.0,
                1_800_000_000.0,
            )
            .unwrap()
            .unwrap();
        assert!(!out.condition);
        assert_eq!(out.held_s, None);
        assert_eq!(out.message, "server web-1 (42): no recent CPU data");
    }

    #[test]
    fn network_and_disk_need_a_threshold_and_metric() {
        assert!(CheckType::Network.validate_params(&json!({})).is_err());
        assert!(
            CheckType::Network
                .validate_params(&json!({ "metric": "bandwidth_out" }))
                .is_err()
        );
        assert!(
            CheckType::Network
                .validate_params(&json!({ "metric": "bandwidth_up", "threshold": 1 }))
                .is_err()
        );
        assert!(
            CheckType::Network
                .validate_params(&json!({ "metric": "bandwidth_out", "threshold": 0 }))
                .is_err()
        );
        assert!(
            CheckType::Network
                .validate_params(&json!({ "metric": "pps_in", "threshold": 1000 }))
                .is_ok()
        );
        assert!(
            CheckType::DiskIo
                .validate_params(&json!({ "metric": "iops_write", "threshold": 500, "for_s": 60 }))
                .is_ok()
        );
        assert!(
            CheckType::Cpu
                .validate_params(&json!({ "threshold_pct": 101 }))
                .is_err()
        );
        assert_eq!(CheckType::Cpu.window_s(&json!({})), 600);
        assert_eq!(
            CheckType::Network.window_s(&json!({ "metric": "pps_in", "threshold": 1 })),
            300
        );
        assert_eq!(CheckType::ServerStatus.window_s(&json!({})), 0);
    }

    #[test]
    fn disk_writes_above_threshold() {
        let now = 1_800_000_000.0;
        let samples: Vec<(f64, f64)> = (0..5)
            .map(|i| (now - 60.0 * f64::from(4 - i), 800.0))
            .collect();
        let metrics = BTreeMap::from([("disk.0.iops.write".to_owned(), samples)]);
        let out = CheckType::DiskIo
            .evaluate_metrics(
                &json!({ "metric": "iops_write", "threshold": 500, "for_s": 300 }),
                &two_core_server(),
                &metrics,
                60.0,
                now,
            )
            .unwrap()
            .unwrap();
        assert!(out.condition);
        assert_eq!(out.held_s, Some(300));
        assert_eq!(
            out.message,
            "server web-1 (42): disk writes 800 IOPS for 5 min (threshold 500 for 5 min)"
        );
    }

    fn eval(check: CheckType, params: &Value, snapshot: &Value, kind: &str) -> Option<Outcome> {
        check.evaluate(params, snapshot, &ctx(kind)).unwrap()
    }

    fn cert(not_valid_after: Option<&str>, status: &Value) -> Value {
        json!({ "id": 5, "name": "api", "type": "managed", "not_valid_after": not_valid_after, "status": status })
    }

    #[test]
    fn certificate_expiry_boundaries() {
        let p = json!({ "days": 14 });
        let ok = json!({ "issuance": "completed", "renewal": "scheduled", "error": null });
        // 2026-12-01 is 18.5 days after NOW: fine.
        let fine = eval(
            CheckType::CertificateExpiry,
            &p,
            &cert(Some("2026-12-01T00:00:00+00:00"), &ok),
            "certificate",
        )
        .unwrap();
        assert!(!fine.condition);
        assert_eq!(fine.value, Some(18.0));
        assert_eq!(
            fine.message,
            "certificate api (5): valid until 2026-12-01 (18 days)"
        );
        // 2026-11-20: 7.5 days left.
        let soon = eval(
            CheckType::CertificateExpiry,
            &p,
            &cert(Some("2026-11-20T00:00:00Z"), &ok),
            "certificate",
        )
        .unwrap();
        assert!(soon.condition);
        assert_eq!(
            soon.message,
            "certificate api (5): expires in 7 days (2026-11-20)"
        );
        // Already expired.
        let gone = eval(
            CheckType::CertificateExpiry,
            &p,
            &cert(Some("2026-11-10T00:00:00Z"), &json!(null)),
            "certificate",
        )
        .unwrap();
        assert!(gone.condition);
        assert_eq!(gone.value, Some(-3.0));
        assert_eq!(gone.message, "certificate api (5): expired on 2026-11-10");
        // Pending issuance: no date, no alert.
        let pending = eval(
            CheckType::CertificateExpiry,
            &p,
            &cert(
                None,
                &json!({ "issuance": "pending", "renewal": "unavailable" }),
            ),
            "certificate",
        )
        .unwrap();
        assert!(!pending.condition);
        assert_eq!(pending.value, None);
    }

    #[test]
    fn failed_renewal_fires_only_with_on_failure() {
        let failed = json!({ "issuance": "completed", "renewal": "failed",
            "error": { "code": "dns_validation_failed", "message": "DNS record not found" } });
        let snap = cert(Some("2027-02-01T00:00:00Z"), &failed);
        let out = eval(
            CheckType::CertificateExpiry,
            &json!({}),
            &snap,
            "certificate",
        )
        .unwrap();
        assert!(out.condition);
        assert_eq!(
            out.message,
            "certificate api (5): renewal failed (DNS record not found); expires in 80 days (2027-02-01)"
        );
        let quiet = eval(
            CheckType::CertificateExpiry,
            &json!({ "days": 3, "on_failure": false }),
            &snap,
            "certificate",
        )
        .unwrap();
        assert!(!quiet.condition);
        assert!(
            CheckType::CertificateExpiry
                .validate_params(&json!({ "days": 0 }))
                .is_err()
        );
        assert!(
            CheckType::CertificateExpiry
                .validate_params(&json!({ "days": 30, "on_failure": false }))
                .is_ok()
        );
    }

    fn typed_server(locations: &Value, top: &Value) -> Value {
        json!({ "id": 1, "name": "web-1", "location": { "name": "fsn1" },
                "server_type": { "name": "cx11", "deprecation": top, "locations": locations } })
    }

    #[test]
    fn deprecation_reads_the_servers_location() {
        let dep = json!({ "announced": "2026-09-01T00:00:00Z", "unavailable_after": "2026-12-01T00:00:00Z" });
        let here = typed_server(
            &json!([{ "name": "fsn1", "deprecation": dep }, { "name": "nbg1", "deprecation": null }]),
            &json!(null),
        );
        let out = eval(CheckType::Deprecation, &json!({}), &here, "server").unwrap();
        assert!(out.condition);
        assert_eq!(out.value, Some(18.0));
        assert_eq!(
            out.message,
            "server web-1 (1): server type cx11 is deprecated in fsn1, unavailable after 2026-12-01 (announced 2026-09-01)"
        );
        // Deprecated elsewhere only: fine.
        let elsewhere = typed_server(
            &json!([{ "name": "fsn1", "deprecation": null }, { "name": "nbg1", "deprecation": dep }]),
            &json!(null),
        );
        let out = eval(CheckType::Deprecation, &json!({}), &elsewhere, "server").unwrap();
        assert!(!out.condition);
        assert_eq!(
            out.message,
            "server web-1 (1): server type cx11 in fsn1 is not deprecated"
        );
        // Fallback: the deprecated top-level field.
        let old = typed_server(&json!([]), &dep);
        assert!(
            eval(CheckType::Deprecation, &json!({}), &old, "server")
                .unwrap()
                .condition
        );
        // Load balancer type.
        let lb = json!({ "id": 3, "name": "lb", "location": { "name": "fsn1" }, "load_balancer_type": { "name": "lb11", "deprecation": dep } });
        let out = eval(CheckType::Deprecation, &json!({}), &lb, "load_balancer").unwrap();
        assert!(out.condition);
        assert_eq!(
            out.message,
            "load balancer lb (3): load balancer type lb11 is deprecated, unavailable after 2026-12-01 (announced 2026-09-01)"
        );
        assert!(
            CheckType::Deprecation
                .validate_params(&json!({ "x": 1 }))
                .is_err()
        );
    }

    #[test]
    fn hygiene_backups_and_protection() {
        let off = json!({ "id": 1, "name": "web-1", "backup_window": null, "protection": { "delete": false, "rebuild": true } });
        let out = eval(CheckType::HygieneBackups, &json!({}), &off, "server").unwrap();
        assert!(out.condition);
        assert_eq!(out.hold_s, 3600);
        assert_eq!(out.message, "server web-1 (1): backups disabled");
        let on = json!({ "id": 1, "name": "web-1", "backup_window": "22-02", "protection": { "delete": true, "rebuild": true } });
        assert!(
            !eval(CheckType::HygieneBackups, &json!({}), &on, "server")
                .unwrap()
                .condition
        );

        let out = eval(CheckType::HygieneProtection, &json!({}), &off, "server").unwrap();
        assert!(out.condition);
        assert_eq!(out.message, "server web-1 (1): delete protection off");
        assert!(
            !eval(CheckType::HygieneProtection, &json!({}), &on, "server")
                .unwrap()
                .condition
        );
        let vol = json!({ "id": 4, "name": "data", "protection": { "delete": false } });
        // Volumes are not in the default kinds.
        assert_eq!(
            eval(CheckType::HygieneProtection, &json!({}), &vol, "volume"),
            None
        );
        let p = json!({ "kinds": ["server", "volume"] });
        let out = eval(CheckType::HygieneProtection, &p, &vol, "volume").unwrap();
        assert_eq!(out.message, "volume data (4): delete protection off");
        let both =
            json!({ "id": 2, "name": "db", "protection": { "delete": false, "rebuild": false } });
        assert_eq!(
            eval(CheckType::HygieneProtection, &json!({}), &both, "server")
                .unwrap()
                .message,
            "server db (2): delete and rebuild protection off"
        );
        assert!(
            CheckType::HygieneProtection
                .validate_params(&json!({ "kinds": ["certificate"] }))
                .is_err()
        );
        assert!(
            CheckType::HygieneProtection
                .validate_params(&json!({ "kinds": [] }))
                .is_err()
        );
    }

    #[test]
    fn orphans_per_kind() {
        let p = json!({});
        let vol = json!({ "id": 4, "name": "data", "server": null, "status": "available" });
        let out = eval(CheckType::HygieneOrphans, &p, &vol, "volume").unwrap();
        assert!(out.condition);
        assert_eq!(out.hold_s, 86_400);
        assert_eq!(out.message, "volume data (4): not attached to a server");
        let creating = json!({ "id": 4, "name": "data", "server": null, "status": "creating" });
        assert!(
            !eval(CheckType::HygieneOrphans, &p, &creating, "volume")
                .unwrap()
                .condition
        );
        let attached = json!({ "id": 4, "name": "data", "server": 1, "status": "available" });
        assert!(
            !eval(CheckType::HygieneOrphans, &p, &attached, "volume")
                .unwrap()
                .condition
        );
        let fip = json!({ "id": 7, "name": "vip", "ip": "203.0.113.7", "server": null });
        assert_eq!(
            eval(CheckType::HygieneOrphans, &p, &fip, "floating_ip")
                .unwrap()
                .message,
            "floating ip vip (7) [203.0.113.7]: not assigned to a server"
        );
        let pip = json!({ "id": 9, "name": "pip", "ip": "198.51.100.9", "assignee_id": 1, "assignee_type": "server" });
        assert!(
            !eval(CheckType::HygieneOrphans, &p, &pip, "primary_ip")
                .unwrap()
                .condition
        );
        let free = json!({ "id": 9, "name": "pip", "ip": "198.51.100.9", "assignee_id": null, "assignee_type": "server" });
        assert!(
            eval(CheckType::HygieneOrphans, &p, &free, "primary_ip")
                .unwrap()
                .condition
        );
    }

    fn point(day: &str, net: f64) -> CostPoint {
        CostPoint {
            day: day.to_owned(),
            monthly_net: net,
            currency: "EUR".to_owned(),
        }
    }

    #[test]
    fn cost_change_against_an_earlier_day() {
        let p = json!({ "threshold_pct": 10 });
        let check = |cur: &CostPoint, prev: Option<&CostPoint>| {
            CheckType::CostChange
                .evaluate_cost(&p, cur, prev)
                .unwrap()
                .unwrap()
        };
        let first = check(&point("2026-11-12", 23.29), None);
        assert!(!first.condition);
        assert_eq!(
            first.message,
            "estimated monthly cost 23.29 EUR (no earlier day to compare yet)"
        );
        let small = check(
            &point("2026-11-12", 25.0),
            Some(&point("2026-11-11", 23.29)),
        );
        assert!(!small.condition);
        assert_eq!(small.value, Some(7.3));
        let big = check(&point("2026-11-12", 30.0), Some(&point("2026-11-11", 25.0)));
        assert!(big.condition);
        assert_eq!(
            big.message,
            "estimated monthly cost 30.00 EUR, +20 % vs. 2026-11-11 (25.00 EUR); net, without traffic overage and snapshots"
        );
        let down = check(&point("2026-11-12", 10.0), Some(&point("2026-11-11", 25.0)));
        assert!(!down.condition);
        assert_eq!(down.value, Some(-60.0));
        assert!(check(&point("2026-11-12", 4.0), Some(&point("2026-11-11", 0.0))).condition);
        assert!(
            CheckType::CostChange
                .validate_params(&json!({ "threshold_pct": 0 }))
                .is_err()
        );
        assert_eq!(
            CheckType::CostChange
                .evaluate(&p, &json!({}), &ctx("server"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn traffic_forecast_extrapolates_to_the_month_end() {
        let tb = 1_000_000_000_000_u64;
        let p = json!({ "threshold_pct": 100, "forecast": true });
        assert!(CheckType::TrafficQuota.inventory_only(&p));
        assert!(!CheckType::TrafficQuota.inventory_only(&json!({ "threshold_pct": 100 })));
        // 300 GB a day ago, 400 GB now → +100 GB/day; 18.5 days left in November → 2.25 TB.
        let snap = traffic(Some(tb * 4 / 10), Some(2 * tb));
        let with = |baseline| Context {
            now: NOW,
            kind: "server",
            traffic_baseline: baseline,
        };
        let base = TrafficSample {
            at: NOW - 86_400,
            outgoing: tb * 3 / 10,
        };
        let out = CheckType::TrafficQuota
            .evaluate(&p, &snap, &with(Some(base)))
            .unwrap()
            .unwrap();
        assert!(out.condition, "{}", out.message);
        assert_eq!(out.value, Some(112.5));
        assert_eq!(
            out.message,
            "web-1 (42): forecast 112.5 % of the included traffic by 2026-11-30 (2250.0 of 2000.0 GB; estimate, assumes the quota resets on the 1st, UTC)"
        );
        // A slower rate stays below.
        let slow = TrafficSample {
            at: NOW - 2 * 86_400,
            outgoing: tb * 3 / 10,
        };
        let out = CheckType::TrafficQuota
            .evaluate(&p, &snap, &with(Some(slow)))
            .unwrap()
            .unwrap();
        assert!(!out.condition);
        assert_eq!(out.value, Some(66.3));
        // Counter went down: reset, no forecast.
        let reset = TrafficSample {
            at: NOW - 86_400,
            outgoing: tb,
        };
        let out = CheckType::TrafficQuota
            .evaluate(&p, &snap, &with(Some(reset)))
            .unwrap()
            .unwrap();
        assert!(!out.condition);
        assert_eq!(out.value, None);
        assert_eq!(
            out.message,
            "web-1 (42): traffic counter was reset since 2026-11-11; forecast resumes in a day"
        );
        // No baseline, or one that is too young.
        for b in [
            None,
            Some(TrafficSample {
                at: NOW - 3600,
                outgoing: 0,
            }),
        ] {
            let out = CheckType::TrafficQuota
                .evaluate(&p, &snap, &with(b))
                .unwrap()
                .unwrap();
            assert!(!out.condition);
            assert_eq!(out.value, None);
        }
    }

    #[test]
    fn month_end_handles_december() {
        // 2026-12-15 → 2027-01-01.
        assert_eq!(next_month_start(1_797_336_000), 1_798_761_600);
        assert_eq!(next_month_start(NOW), 1_796_083_200); // 2026-12-01
    }

    #[test]
    fn inventory_checks_are_inventory_only() {
        for check in [
            CheckType::CertificateExpiry,
            CheckType::Deprecation,
            CheckType::HygieneBackups,
            CheckType::HygieneProtection,
            CheckType::HygieneOrphans,
            CheckType::CostChange,
        ] {
            assert!(check.inventory_only(&json!({})), "{check}");
            assert!(check.validate_params(&json!({})).is_ok(), "{check}");
        }
        assert!(!CheckType::ServerStatus.inventory_only(&json!({})));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(duration(3600), "1 h");
        assert_eq!(duration(3900), "1 h 5 min");
    }
}
