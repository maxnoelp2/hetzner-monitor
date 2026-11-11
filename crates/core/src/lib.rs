//! Domain logic for hetzner-monitor: checks, rule evaluation, alert state machine,
//! label selectors, secret handling and budget maths.
//!
//! Keep this crate free of IO where possible so it stays unit-testable.
//! See `ARCHITECTURE.md` and `docs/design/`.

pub mod alert;
pub mod apikey;
pub mod budget;
pub mod checks;
pub mod crypto;
pub mod secret;
pub mod selector;
