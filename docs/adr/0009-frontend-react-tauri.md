# ADR-0009: React PWA, packaged with Tauri 2 for desktop and mobile

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

Surfaces wanted: web, mobile, desktop. Separate native codebases are too expensive for a small team.

## Decision

Build one React PWA (after the headless core). Package it with Tauri 2 for iOS/Android and desktop. Order: PWA → mobile → desktop.

## Consequences

- **Pro:** One UI codebase; Tauri is Rust, matching the core.
- **Con:** Native push (APNs/FCM) needs Tauri plugins; Web Push covers the PWA phase.
