//! Alert state machine, one instance per (rule, resource). Pure; time is UNIX seconds.
//! See docs/design/alerting.md#alert-state-machine.
//!
//! ```text
//! ok ──cond──▶ pending ──held ≥ hold_s──▶ firing ──¬cond for RESOLVE_AFTER cycles──▶ resolved
//!  ▲             │ ¬cond                    ▲ │
//!  └─────────────┘ (never fired: dropped)   └─┘ cond again: counter resets
//! ```

/// Consecutive good cycles needed before a firing alert resolves (flapping guard).
pub const RESOLVE_AFTER: u32 = 2;

/// The open (not yet resolved) alert of a (rule, resource), if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    Pending { since: i64 },
    Firing { ok_cycles: u32 },
}

/// What happens to the alert in this cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// No open alert and the condition is false.
    Nothing,
    /// Open a new `pending` alert.
    OpenPending,
    /// Open a new alert directly as `firing` (no hold time).
    OpenFiring,
    /// Still pending; hold time not reached yet.
    StayPending,
    /// `pending → firing`.
    Fire,
    /// `pending → ok`: the alert never fired and is dropped.
    Cancel,
    /// Still firing; `ok_cycles` is the new value of the flapping counter.
    StayFiring { ok_cycles: u32 },
    /// `firing → resolved`.
    Resolve,
}

/// Next transition for a condition evaluated at `now`, with the condition having to hold
/// for `hold_s` seconds before firing.
///
/// `measured_s` is how long the condition has held as measured by the check itself (metric checks
/// measure it on Hetzner's series). Without it, the alert's local `pending` time is used.
pub fn step(
    open: Option<Open>,
    condition: bool,
    hold_s: u64,
    measured_s: Option<u64>,
    now: i64,
) -> Transition {
    let held_long_enough = |since: i64| {
        measured_s.unwrap_or_else(|| u64::try_from(now.saturating_sub(since)).unwrap_or(0))
            >= hold_s
    };
    match (open, condition) {
        (None, false) => Transition::Nothing,
        (None, true) if hold_s == 0 || measured_s.is_some_and(|h| h >= hold_s) => {
            Transition::OpenFiring
        }
        (None, true) => Transition::OpenPending,
        (Some(Open::Pending { since }), true) if held_long_enough(since) => Transition::Fire,
        (Some(Open::Pending { .. }), true) => Transition::StayPending,
        (Some(Open::Pending { .. }), false) => Transition::Cancel,
        (Some(Open::Firing { .. }), true) => Transition::StayFiring { ok_cycles: 0 },
        (Some(Open::Firing { ok_cycles }), false) if ok_cycles + 1 >= RESOLVE_AFTER => {
            Transition::Resolve
        }
        (Some(Open::Firing { ok_cycles }), false) => Transition::StayFiring {
            ok_cycles: ok_cycles + 1,
        },
    }
}

/// Transition for an open alert whose resource disappeared or left the rule's scope:
/// resolve immediately (no flapping guard — there is nothing left to observe).
pub const fn gone(open: Open) -> Transition {
    match open {
        Open::Pending { .. } => Transition::Cancel,
        Open::Firing { .. } => Transition::Resolve,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000;

    #[test]
    fn ok_stays_ok() {
        assert_eq!(step(None, false, 120, None, T0), Transition::Nothing);
    }

    #[test]
    fn condition_opens_pending_then_fires_after_hold() {
        assert_eq!(step(None, true, 120, None, T0), Transition::OpenPending);
        let pending = Some(Open::Pending { since: T0 });
        assert_eq!(
            step(pending, true, 120, None, T0 + 60),
            Transition::StayPending
        );
        assert_eq!(step(pending, true, 120, None, T0 + 120), Transition::Fire);
    }

    #[test]
    fn zero_hold_fires_at_once() {
        assert_eq!(step(None, true, 0, None, T0), Transition::OpenFiring);
    }

    #[test]
    fn pending_is_dropped_when_condition_clears() {
        let pending = Some(Open::Pending { since: T0 });
        assert_eq!(step(pending, false, 120, None, T0 + 60), Transition::Cancel);
    }

    #[test]
    fn firing_resolves_only_after_two_good_cycles() {
        let firing = Some(Open::Firing { ok_cycles: 0 });
        assert_eq!(
            step(firing, false, 120, None, T0),
            Transition::StayFiring { ok_cycles: 1 }
        );
        let once_ok = Some(Open::Firing { ok_cycles: 1 });
        assert_eq!(step(once_ok, false, 120, None, T0), Transition::Resolve);
    }

    #[test]
    fn flapping_resets_the_counter() {
        let once_ok = Some(Open::Firing { ok_cycles: 1 });
        assert_eq!(
            step(once_ok, true, 120, None, T0),
            Transition::StayFiring { ok_cycles: 0 }
        );
    }

    #[test]
    fn clock_going_backwards_does_not_fire() {
        let pending = Some(Open::Pending { since: T0 });
        assert_eq!(
            step(pending, true, 120, None, T0 - 500),
            Transition::StayPending
        );
    }

    #[test]
    fn gone_resources_close_their_alert() {
        assert_eq!(gone(Open::Pending { since: T0 }), Transition::Cancel);
        assert_eq!(gone(Open::Firing { ok_cycles: 0 }), Transition::Resolve);
    }

    #[test]
    fn measured_duration_overrides_the_local_clock() {
        // Metric checks: 10 min measured on the series fire at once, even for a new alert.
        assert_eq!(step(None, true, 600, Some(600), T0), Transition::OpenFiring);
        assert_eq!(
            step(None, true, 600, Some(300), T0),
            Transition::OpenPending
        );
        let pending = Some(Open::Pending { since: T0 });
        // Local time would say 1000 s, the series says 300 s: not yet.
        assert_eq!(
            step(pending, true, 600, Some(300), T0 + 1000),
            Transition::StayPending
        );
        assert_eq!(
            step(pending, true, 600, Some(660), T0 + 60),
            Transition::Fire
        );
    }
}
