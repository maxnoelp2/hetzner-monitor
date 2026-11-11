//! Minimal client for the Hetzner Cloud API.
//!
//! **Read-only by design:** this crate implements `GET` requests only and must never gain
//! `POST`, `PUT` or `DELETE` support (ARCHITECTURE.md, invariant 1). Callers account every
//! request against the project's rate-limit budget using the returned [`RateLimit`].
//! See `docs/design/hcloud-client.md`.

mod client;
mod error;
mod model;

pub use client::{Client, DEFAULT_BASE_URL};
pub use error::Error;
pub use model::{Listed, MetricType, Metrics, RateLimit, RawResource, ResourceKind, Token};
