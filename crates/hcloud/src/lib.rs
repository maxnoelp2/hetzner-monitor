//! Minimal client for the Hetzner Cloud API.
//!
//! **Read-only by design:** this crate implements `GET` requests only and must never gain
//! `POST`, `PUT` or `DELETE` support. Every request goes through rate-limit budget accounting.
//! See `docs/design/hcloud-client.md`. Implementation arrives in M1.
