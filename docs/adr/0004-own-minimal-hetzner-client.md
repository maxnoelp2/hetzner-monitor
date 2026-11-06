# ADR-0004: Hand-written minimal Hetzner client

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Official SDKs exist only for Go and Python. Options: community crate, OpenAPI-generated client, or own client.

## Decision

Write a small client (`crates/hcloud`) with `reqwest` + `serde`, covering only the GET endpoints we use, with first-class rate-limit header capture and typed errors.

## Consequences

- **Pro:** Full control over rate limiting, pagination (`meta.pagination`, not the deprecated `Link` header) and error mapping.
- **Pro:** Read-only by construction.
- **Con:** We maintain it and must track API changes ([changelog](https://docs.hetzner.cloud/changelog)).
