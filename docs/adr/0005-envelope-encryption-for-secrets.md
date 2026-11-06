# ADR-0005: Envelope encryption (AES-256-GCM) for secrets at rest

- Status: Accepted
- Date: 2026-11-05
- Deciders: Max Nölp

## Context

The database stores tokens for other people's infrastructure and webhook URLs that grant posting rights. A DB dump must not expose them.

## Decision

Encrypt each secret with a random per-row DEK (AES-256-GCM, AAD = row id); encrypt the DEK with a versioned master key from `HM_MASTER_KEY(_FILE)`. Details: [security](../design/security.md#envelope-encryption-for-secrets-at-rest).

## Consequences

- **Pro:** DB dump alone is useless; master-key rotation only re-wraps DEKs; KMS can replace the master key in the SaaS.
- **Con:** Losing the master key loses all stored tokens. Rotation tooling is still **proposed**.
