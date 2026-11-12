//! In-process login throttling (ADR-0013): failed logins are counted per e-mail and per client
//! address in a sliding window. One API process per installation is the self-hosted case; a
//! multi-process deployment gets one counter per process.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Sliding window for counting failures.
pub const WINDOW: Duration = Duration::from_mins(15);
/// Failures per e-mail address within [`WINDOW`] before logins for it are refused.
pub const MAX_PER_EMAIL: usize = 10;
/// Failures per client address within [`WINDOW`] before its logins are refused.
pub const MAX_PER_ADDR: usize = 30;
/// Above this many tracked keys, stale ones are dropped on the next failure.
const PRUNE_ABOVE: usize = 10_000;

/// Counts failed logins. Keys are `e:<email>` and `a:<address>`.
#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

fn keys(email: &str, addr: Option<&str>) -> [(Option<String>, usize); 2] {
    [
        (Some(format!("e:{}", email.to_lowercase())), MAX_PER_EMAIL),
        (addr.map(|a| format!("a:{a}")), MAX_PER_ADDR),
    ]
}

impl LoginThrottle {
    /// `Some(retry_after)` if a login for `email` from `addr` must be refused at `now`.
    pub fn check(&self, email: &str, addr: Option<&str>, now: Instant) -> Option<Duration> {
        let mut map = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut wait = None;
        for (key, max) in keys(email, addr) {
            let Some(list) = key.and_then(|k| map.get_mut(&k)) else {
                continue;
            };
            while list
                .front()
                .is_some_and(|t| now.duration_since(*t) >= WINDOW)
            {
                list.pop_front();
            }
            if list.len() >= max {
                let oldest = *list.front().expect("non-empty");
                let retry = WINDOW.saturating_sub(now.duration_since(oldest));
                wait = Some(wait.map_or(retry, |w: Duration| w.max(retry)));
            }
        }
        wait
    }

    /// Records one failed login.
    pub fn fail(&self, email: &str, addr: Option<&str>, now: Instant) {
        let mut map = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if map.len() > PRUNE_ABOVE {
            map.retain(|_, list| list.back().is_some_and(|t| now.duration_since(*t) < WINDOW));
        }
        for (key, _) in keys(email, addr) {
            if let Some(key) = key {
                map.entry(key).or_default().push_back(now);
            }
        }
    }

    /// Forgets the failures of `email` after a successful login (the address keeps its count).
    pub fn succeed(&self, email: &str) {
        let mut map = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.remove(&format!("e:{}", email.to_lowercase()));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{LoginThrottle, MAX_PER_ADDR, MAX_PER_EMAIL, WINDOW};

    #[test]
    fn per_email_limit_and_window() {
        let t = LoginThrottle::default();
        let start = Instant::now();
        for _ in 0..MAX_PER_EMAIL {
            assert_eq!(t.check("A@x.io", Some("1.2.3.4"), start), None);
            t.fail("A@x.io", Some("1.2.3.4"), start);
        }
        // Case-insensitive, any address.
        assert_eq!(t.check("a@x.io", None, start), Some(WINDOW));
        assert_eq!(t.check("b@x.io", Some("1.2.3.4"), start), None);
        let later = start + WINDOW;
        assert_eq!(t.check("a@x.io", None, later), None);
    }

    #[test]
    fn per_address_limit_and_success_reset() {
        let t = LoginThrottle::default();
        let now = Instant::now();
        for i in 0..MAX_PER_ADDR {
            t.fail(&format!("u{i}@x.io"), Some("9.9.9.9"), now);
        }
        let retry = t.check("new@x.io", Some("9.9.9.9"), now + Duration::from_secs(60));
        assert_eq!(retry, WINDOW.checked_sub(Duration::from_secs(60)));
        assert_eq!(t.check("new@x.io", Some("8.8.8.8"), now), None);

        let t = LoginThrottle::default();
        for _ in 0..MAX_PER_EMAIL {
            t.fail("a@x.io", None, now);
        }
        t.succeed("a@x.io");
        assert_eq!(t.check("a@x.io", None, now), None);
    }
}
