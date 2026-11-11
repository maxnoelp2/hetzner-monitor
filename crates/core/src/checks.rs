//! Check implementations (docs/design/checks.md). Pure: a check reads a Hetzner resource
//! snapshot (the JSON object as returned by the API) and its rule params, and returns an
//! [`Outcome`]. Field shapes follow the Hetzner `OpenAPI` spec (verified 2026-11-11).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::series;

/// Upper bound for durations in params (one day).
const MAX_DURATION_S: u64 = 86_400;

/// The checks known to M2. Stored as `rules.check_type`.
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
}

impl CheckType {
    pub const ALL: [Self; 8] = [
        Self::ServerStatus,
        Self::ResourceLifecycle,
        Self::LbTargetHealth,
        Self::TrafficQuota,
        Self::HetznerApi,
        Self::Cpu,
        Self::Network,
        Self::DiskIo,
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
            Self::ServerStatus | Self::Cpu | Self::Network | Self::DiskIo => &["server"],
            Self::LbTargetHealth => &["load_balancer"],
            Self::TrafficQuota => &["server", "load_balancer"],
            Self::ResourceLifecycle | Self::HetznerApi => &[],
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
        }
    }

    /// Evaluates a per-resource check on `snapshot`. `None` for checks that are not
    /// evaluated per resource (`resource_lifecycle`, `hetzner_api`).
    pub fn evaluate(
        self,
        params: &Value,
        snapshot: &Value,
    ) -> Result<Option<Outcome>, ParamsError> {
        Ok(match self {
            Self::ServerStatus => Some(server_status(snapshot, &parse(params)?)),
            Self::LbTargetHealth => Some(lb_target_health(snapshot, &parse(params)?)),
            Self::TrafficQuota => Some(traffic_quota(snapshot, &parse(params)?)),
            Self::ResourceLifecycle
            | Self::HetznerApi
            | Self::Cpu
            | Self::Network
            | Self::DiskIo => None,
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

/// `traffic_quota`: `outgoing_traffic / included_traffic` (current billing period) ≥ threshold.
/// Without both numbers (Hetzner returns `null` until there is data) the condition is false.
pub fn traffic_quota(snapshot: &Value, params: &TrafficQuotaParams) -> Outcome {
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

    #[test]
    fn traffic_below_and_above_threshold() {
        let p: TrafficQuotaParams = parse(&json!({ "threshold_pct": 80 })).unwrap();
        let tb = 1_000_000_000_000_u64;
        let low = traffic_quota(&traffic(Some(tb / 2), Some(tb)), &p);
        assert!(!low.condition);
        assert_eq!(low.value, Some(50.0));
        let high = traffic_quota(&traffic(Some(tb * 8 / 10), Some(tb)), &p);
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
        assert!(!traffic_quota(&traffic(None, Some(1)), &p).condition);
        assert!(!traffic_quota(&traffic(Some(5), None), &p).condition);
        assert!(!traffic_quota(&traffic(Some(5), Some(0)), &p).condition);
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

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(600), "10 min");
        assert_eq!(duration(3600), "1 h");
        assert_eq!(duration(3900), "1 h 5 min");
    }
}
