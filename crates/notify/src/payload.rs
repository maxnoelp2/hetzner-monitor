//! The one internal notification payload, rendered per channel. Its JSON form is the
//! documented webhook schema, `version: 1` (docs/design/alerting.md#webhook-payload).

use std::fmt;
use std::str::FromStr;

use serde::Serialize;

/// Webhook schema version. Bump only with a documented, breaking change.
pub const PAYLOAD_VERSION: u32 = 1;

/// What happened to the alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    Fired,
    Resolved,
    Renotified,
    /// `POST /channels/{id}/test`.
    Test,
}

impl Event {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fired => "fired",
            Self::Resolved => "resolved",
            Self::Renotified => "renotified",
            Self::Test => "test",
        }
    }
}

impl FromStr for Event {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fired" => Ok(Self::Fired),
            "resolved" => Ok(Self::Resolved),
            "renotified" => Ok(Self::Renotified),
            "test" => Ok(Self::Test),
            other => Err(format!("unknown notification event `{other}`")),
        }
    }
}

/// Alert severity, ordered `info < warning < critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "critical" => Ok(Self::Critical),
            other => Err(format!(
                "severity must be info, warning or critical, got `{other}`"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AlertInfo {
    pub id: String,
    /// `pending | firing | resolved` at the time of the event.
    pub state: String,
    pub severity: Severity,
    pub check_type: String,
    pub message: String,
    pub value: Option<f64>,
    /// RFC 3339, UTC.
    pub fired_at: Option<String>,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Named {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResourceInfo {
    /// `server | load_balancer | volume | certificate | floating_ip | primary_ip`.
    pub kind: String,
    pub hetzner_id: i64,
    pub name: String,
}

/// A notification. Serialised as-is for webhooks.
#[derive(Debug, Clone, Serialize)]
pub struct Payload {
    pub version: u32,
    pub event: Event,
    /// When the event happened, RFC 3339 UTC.
    pub at: String,
    pub alert: AlertInfo,
    pub rule: Named,
    pub project: Named,
    pub resource: Option<ResourceInfo>,
    /// Link to the alert (`HM_BASE_URL`), if configured.
    pub link: Option<String>,
}

impl Payload {
    /// The payload of a channel test. Uses placeholder ids so receivers can recognise it.
    pub fn test(channel_name: &str, at: &str) -> Self {
        Self {
            version: PAYLOAD_VERSION,
            event: Event::Test,
            at: at.to_owned(),
            alert: AlertInfo {
                id: "00000000-0000-0000-0000-000000000000".to_owned(),
                state: "firing".to_owned(),
                severity: Severity::Info,
                check_type: "test".to_owned(),
                message: format!(
                    "Test notification for channel \"{channel_name}\". If you can read this, the channel works."
                ),
                value: None,
                fired_at: Some(at.to_owned()),
                resolved_at: None,
            },
            rule: Named {
                id: "00000000-0000-0000-0000-000000000000".to_owned(),
                name: "Channel test".to_owned(),
            },
            project: Named {
                id: "00000000-0000-0000-0000-000000000000".to_owned(),
                name: "hetzner-monitor".to_owned(),
            },
            resource: None,
            link: None,
        }
    }

    /// One-line summary, e.g. `FIRING [critical] Server not running: server web-1`.
    pub fn title(&self) -> String {
        let label = match self.event {
            Event::Fired => "FIRING",
            Event::Resolved => "RESOLVED",
            Event::Renotified => "STILL FIRING",
            Event::Test => "TEST",
        };
        let subject = self.resource.as_ref().map_or_else(
            || format!("project {}", self.project.name),
            |r| format!("{} {}", r.kind.replace('_', " "), r.name),
        );
        format!(
            "{label} [{}] {}: {subject}",
            self.alert.severity, self.rule.name
        )
    }

    /// Plain-text body: the alert message, the project and the link.
    pub fn text(&self) -> String {
        let mut out = self.alert.message.clone();
        if self.event != Event::Test {
            out.push_str("\nProject: ");
            out.push_str(&self.project.name);
        }
        if let Some(link) = &self.link {
            out.push('\n');
            out.push_str(link);
        }
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample(event: Event) -> Payload {
        Payload {
            version: PAYLOAD_VERSION,
            event,
            at: "2026-11-12T12:00:00Z".to_owned(),
            alert: AlertInfo {
                id: "a1".to_owned(),
                state: "firing".to_owned(),
                severity: Severity::Critical,
                check_type: "server_status".to_owned(),
                message: "server web-1 (1) is off".to_owned(),
                value: None,
                fired_at: Some("2026-11-12T12:00:00Z".to_owned()),
                resolved_at: None,
            },
            rule: Named {
                id: "r1".to_owned(),
                name: "Server not running".to_owned(),
            },
            project: Named {
                id: "p1".to_owned(),
                name: "prod".to_owned(),
            },
            resource: Some(ResourceInfo {
                kind: "server".to_owned(),
                hetzner_id: 1,
                name: "web-1".to_owned(),
            }),
            link: Some("https://hm.example/api/v1/alerts/a1".to_owned()),
        }
    }

    #[test]
    fn title_and_text() {
        let p = sample(Event::Fired);
        assert_eq!(
            p.title(),
            "FIRING [critical] Server not running: server web-1"
        );
        assert_eq!(
            p.text(),
            "server web-1 (1) is off\nProject: prod\nhttps://hm.example/api/v1/alerts/a1"
        );
        assert_eq!(
            sample(Event::Renotified).title(),
            "STILL FIRING [critical] Server not running: server web-1"
        );
    }

    #[test]
    fn severity_orders_and_parses() {
        assert!(Severity::Info < Severity::Warning && Severity::Warning < Severity::Critical);
        assert_eq!("warning".parse::<Severity>(), Ok(Severity::Warning));
        assert!("high".parse::<Severity>().is_err());
    }

    #[test]
    fn webhook_schema_shape() {
        let v = serde_json::to_value(sample(Event::Resolved)).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["event"], "resolved");
        assert_eq!(v["alert"]["severity"], "critical");
        assert_eq!(v["resource"]["kind"], "server");
    }
}
