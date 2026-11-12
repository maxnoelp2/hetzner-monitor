//! Notification timing (ADR-0012): when a firing alert re-notifies and how a failed delivery
//! backs off. Pure functions, times as Unix seconds.

/// A job is marked `failed` after this many attempts.
pub const MAX_ATTEMPTS: u32 = 8;
/// First retry delay.
pub const BACKOFF_BASE_S: u64 = 30;
/// Longest retry delay.
pub const BACKOFF_MAX_S: u64 = 3600;

/// Whether a still-firing alert re-notifies now: only `warning`/`critical`, only while not
/// acknowledged, once `renotify_s` has passed since the last notification.
pub fn renotify_due(
    severity: &str,
    acked: bool,
    last_notified_at: Option<i64>,
    renotify_s: i64,
    now: i64,
) -> bool {
    if severity == "info" || acked || renotify_s <= 0 {
        return false;
    }
    last_notified_at.is_some_and(|last| now - last >= renotify_s)
}

/// Delay before attempt `attempts + 1` after `attempts` failed ones: 30 s · 2ⁿ⁻¹, capped at
/// 1 h. A server's `Retry-After` (429) is used instead when given, within 1 s … 1 h.
pub fn backoff_s(attempts: u32, retry_after_s: Option<u64>) -> u64 {
    let exp = attempts.saturating_sub(1).min(16);
    let base = BACKOFF_BASE_S.saturating_mul(1 << exp).min(BACKOFF_MAX_S);
    retry_after_s.map_or(base, |r| r.clamp(1, BACKOFF_MAX_S))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renotify_only_for_unacked_warning_and_critical() {
        let h4 = 4 * 3600;
        assert!(renotify_due("critical", false, Some(0), h4, h4));
        assert!(!renotify_due("critical", false, Some(0), h4, h4 - 1));
        assert!(!renotify_due("critical", true, Some(0), h4, 10 * h4));
        assert!(!renotify_due("info", false, Some(0), h4, 10 * h4));
        assert!(renotify_due("warning", false, Some(100), h4, 100 + h4));
        // Never notified (e.g. fired while silenced): nothing to repeat.
        assert!(!renotify_due("critical", false, None, h4, 10 * h4));
    }

    #[test]
    fn backoff_doubles_up_to_an_hour_and_honours_retry_after() {
        assert_eq!(backoff_s(1, None), 30);
        assert_eq!(backoff_s(2, None), 60);
        assert_eq!(backoff_s(3, None), 120);
        assert_eq!(backoff_s(8, None), 3600);
        assert_eq!(backoff_s(40, None), 3600);
        assert_eq!(backoff_s(1, Some(300)), 300);
        assert_eq!(backoff_s(1, Some(0)), 1);
        assert_eq!(backoff_s(1, Some(99_999)), 3600);
    }
}
