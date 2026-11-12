//! Domain logic for hetzner-monitor: checks, rule evaluation, alert state machine,
//! label selectors, secret handling, budget maths and notification timing.
//!
//! Keep this crate free of IO where possible so it stays unit-testable.
//! See `ARCHITECTURE.md` and `docs/design/`.

pub mod alert;
pub mod apikey;
pub mod budget;
pub mod checks;
pub mod cost;
pub mod crypto;
pub mod delivery;
pub mod password;
pub mod secret;
pub mod selector;
pub mod series;
pub mod session;
