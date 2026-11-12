//! The hetzner-monitor service: configuration, database, HTTP API, scheduler, rule engine, notifier.
//!
//! The binary in `main.rs` wires these together; integration tests use them directly.

pub mod api;
pub mod auth;
pub mod bootstrap;
pub mod config;
pub mod cycle;
pub mod db;
pub mod engine;
pub mod error;
pub mod http;
pub mod inventory;
pub mod notifier;
pub mod repo;
pub mod scheduler;
pub mod telemetry;
