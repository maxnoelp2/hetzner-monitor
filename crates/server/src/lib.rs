//! The hetzner-monitor service: configuration, database, HTTP API, inventory sync.
//!
//! The binary in `main.rs` wires these together; integration tests use them directly.

pub mod api;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod db;
pub mod error;
pub mod http;
pub mod inventory;
pub mod repo;
pub mod telemetry;
