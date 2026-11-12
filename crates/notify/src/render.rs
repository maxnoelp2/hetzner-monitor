//! Per-channel message bodies built from a [`Payload`].

use std::fmt::Write as _;

use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use crate::payload::{Event, Payload, Severity};

/// Discord rejects `content` longer than this (characters).
pub const DISCORD_MAX_CHARS: usize = 2000;

/// Slack incoming webhook: `{ "text" }` in mrkdwn, with `&`, `<`, `>` escaped.
pub fn slack(p: &Payload) -> Value {
    let mut text = format!(
        "*{}*\n{}\nProject: {}",
        slack_escape(&p.title()),
        slack_escape(&p.alert.message),
        slack_escape(&p.project.name)
    );
    if let Some(link) = &p.link {
        let _ = write!(text, "\n<{}|Open alert>", slack_escape(link));
    }
    json!({ "text": text })
}

fn slack_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Discord webhook: `content` (≤ 2000 chars); mentions are disabled so alert text can never
/// ping `@everyone` or a role.
pub fn discord(p: &Payload) -> Value {
    let content = format!("**{}**\n{}", p.title(), p.text());
    json!({ "content": truncate(&content, DISCORD_MAX_CHARS), "allowed_mentions": { "parse": [] } })
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// ntfy JSON publish (`POST <server>/` with the topic in the body).
pub fn ntfy(p: &Payload, topic: &str) -> Value {
    let (priority, tag) = match (p.event, p.alert.severity) {
        (Event::Resolved, _) => (3, "white_check_mark"),
        (Event::Test, _) => (3, "test_tube"),
        (_, Severity::Critical) => (5, "rotating_light"),
        (_, Severity::Warning) => (4, "warning"),
        (_, Severity::Info) => (3, "information_source"),
    };
    let mut body = json!({
        "topic": topic,
        "title": p.title(),
        "message": p.text(),
        "priority": priority,
        "tags": [tag],
    });
    if let Some(link) = &p.link {
        body["click"] = json!(link);
    }
    body
}

/// Webhook body: the payload as JSON (schema version 1).
pub fn webhook(p: &Payload) -> Vec<u8> {
    serde_json::to_vec(p).unwrap_or_default()
}

/// `sha256=<hex HMAC-SHA256(secret, body)>` for `X-HM-Signature`.
pub fn signature(secret: &[u8], body: &[u8]) -> String {
    let mut mac =
        <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256=");
    for b in digest {
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::tests::sample;

    #[test]
    fn slack_escapes_and_links() {
        let mut p = sample(Event::Fired);
        p.alert.message = "a < b & c".to_owned();
        let text = slack(&p)["text"].as_str().unwrap().to_owned();
        assert!(text.starts_with("*FIRING [critical] Server not running: server web-1*\n"));
        assert!(text.contains("a &lt; b &amp; c"));
        assert!(text.ends_with("<https://hm.example/api/v1/alerts/a1|Open alert>"));
    }

    #[test]
    fn discord_truncates_and_blocks_mentions() {
        let mut p = sample(Event::Fired);
        p.alert.message = "@everyone ".repeat(500);
        let v = discord(&p);
        assert_eq!(
            v["content"].as_str().unwrap().chars().count(),
            DISCORD_MAX_CHARS
        );
        assert_eq!(v["allowed_mentions"]["parse"], json!([]));
    }

    #[test]
    fn ntfy_priority_follows_severity_and_event() {
        let v = ntfy(&sample(Event::Fired), "t");
        assert_eq!(
            (v["priority"].clone(), v["tags"][0].clone()),
            (json!(5), json!("rotating_light"))
        );
        assert_eq!(v["click"], "https://hm.example/api/v1/alerts/a1");
        assert_eq!(ntfy(&sample(Event::Resolved), "t")["priority"], 3);
    }

    #[test]
    fn signature_matches_a_known_hmac() {
        // RFC 4231 test case 2.
        assert_eq!(
            signature(b"Jefe", b"what do ya want for nothing?"),
            "sha256=5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }
}
