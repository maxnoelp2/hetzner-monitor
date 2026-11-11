//! Evaluating rule durations on Hetzner metric series (docs/design/scheduler.md#metric-query-window).
//! Nothing is stored locally (invariant 8): every evaluation looks at the window just fetched.

/// Samples `(unix_ts, value)`, sorted by time.
pub type Samples = [(f64, f64)];

/// Margin added to the query window for samples that arrive late.
pub const WINDOW_MARGIN_S: i64 = 120;
/// Requested resolution.
pub const STEP_S: u32 = 60;

/// Query window `[start, end]` (UNIX seconds) for a metrics cycle at `now`.
pub fn window(now: i64, interval_s: u32, longest_for_s: u64) -> (i64, i64) {
    let span = i64::try_from(longest_for_s.max(u64::from(interval_s))).unwrap_or(i64::MAX / 2);
    (now - span - WINDOW_MARGIN_S, now)
}

/// The trailing run of a series against a condition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trailing {
    /// Value of the newest sample.
    pub latest: f64,
    /// True if the newest sample meets the condition.
    pub breaching: bool,
    /// How long (seconds) the condition has held without interruption, measured on the series.
    pub held_s: u64,
}

/// Looks at the newest samples: does the condition hold, and for how long?
///
/// - Returns `None` without recent data: an empty series, or a newest sample older than
///   `max(3 × step, 5 min)` (e.g. a powered-off server stops reporting). No data never fires.
/// - A gap larger than 2 × `step` between samples ends the run.
/// - `held_s` counts each sample as one `step`: 10 breaching samples at step 60 = 600 s.
pub fn trailing(
    samples: &Samples,
    step: f64,
    now: f64,
    condition: impl Fn(f64) -> bool,
) -> Option<Trailing> {
    let step = if step.is_finite() && step > 0.0 {
        step
    } else {
        f64::from(STEP_S)
    };
    let &(latest_ts, latest) = samples.last()?;
    if now - latest_ts > (3.0 * step).max(300.0) {
        return None;
    }
    if !condition(latest) {
        return Some(Trailing {
            latest,
            breaching: false,
            held_s: 0,
        });
    }
    let mut run_start = latest_ts;
    for pair in samples.windows(2).rev() {
        let (prev, next) = (pair[0], pair[1]);
        if next.0 - prev.0 > 2.0 * step || !condition(prev.1) {
            break;
        }
        run_start = prev.0;
    }
    // Bounded by the 30-day retention, so the conversion cannot overflow.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let held_s = (latest_ts - run_start + step).max(0.0).round() as u64;
    Some(Trailing {
        latest,
        breaching: true,
        held_s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_800_000_000.0;

    /// `n` samples ending at `NOW`, one per minute, all with value `v`.
    fn flat(n: usize, v: f64) -> Vec<(f64, f64)> {
        (0..n)
            .map(|i| (NOW - 60.0 * f64::from(u32::try_from(n - 1 - i).unwrap()), v))
            .collect()
    }

    #[test]
    fn window_covers_the_longest_rule_plus_margin() {
        assert_eq!(window(10_000, 60, 600), (10_000 - 600 - 120, 10_000));
        assert_eq!(window(10_000, 900, 600), (10_000 - 900 - 120, 10_000));
    }

    #[test]
    fn ten_breaching_minutes_hold_600_s() {
        let t = trailing(&flat(10, 95.0), 60.0, NOW, |v| v > 90.0).unwrap();
        assert!(t.breaching);
        assert_eq!(t.held_s, 600);
        assert!((t.latest - 95.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_run_stops_at_the_first_good_sample() {
        let mut s = flat(10, 95.0);
        s[6].1 = 50.0; // samples 7..9 breach → 3 minutes
        let t = trailing(&s, 60.0, NOW, |v| v > 90.0).unwrap();
        assert_eq!(t.held_s, 180);
    }

    #[test]
    fn a_good_latest_sample_is_not_breaching() {
        let mut s = flat(10, 95.0);
        s[9].1 = 10.0;
        let t = trailing(&s, 60.0, NOW, |v| v > 90.0).unwrap();
        assert!(!t.breaching);
        assert_eq!(t.held_s, 0);
    }

    #[test]
    fn gaps_break_the_run() {
        let mut s = flat(10, 95.0);
        s.drain(5..8); // 4-minute gap; only the last 2 samples follow it
        let t = trailing(&s, 60.0, NOW, |v| v > 90.0).unwrap();
        assert_eq!(t.held_s, 120);
    }

    #[test]
    fn no_or_stale_data_is_none() {
        assert_eq!(trailing(&[], 60.0, NOW, |_| true), None);
        let stale = flat(10, 95.0);
        assert_eq!(trailing(&stale, 60.0, NOW + 301.0, |v| v > 90.0), None);
        assert!(trailing(&stale, 60.0, NOW + 299.0, |v| v > 90.0).is_some());
    }

    #[test]
    fn coarser_steps_count_per_sample() {
        let s: Vec<(f64, f64)> = (0..4)
            .map(|i| (NOW - 300.0 * f64::from(3 - i), 99.0))
            .collect();
        let t = trailing(&s, 300.0, NOW, |v| v > 90.0).unwrap();
        assert_eq!(t.held_s, 1200);
    }

    #[test]
    fn invalid_step_falls_back_to_60() {
        let t = trailing(&flat(2, 95.0), 0.0, NOW, |v| v > 90.0).unwrap();
        assert_eq!(t.held_s, 120);
    }
}
