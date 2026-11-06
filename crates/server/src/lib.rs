//! The hetzner-monitor service: configuration, database, HTTP API.
//!
//! The binary in `main.rs` wires these together; integration tests use them directly.

pub mod config;
pub mod db;
pub mod http;
pub mod telemetry;
