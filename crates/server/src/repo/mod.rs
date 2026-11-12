//! Database access. **Every function on tenant data takes an [`OrgId`]** and filters by it
//! (ARCHITECTURE.md, invariant 5). The only exceptions are API key lookup by prefix, which
//! happens before the tenant is known and returns the tenant, and the scheduler's
//! [`projects::claim_due`], which returns each claimed project with its tenant.

pub mod alerts;
pub mod api_keys;
pub mod projects;
pub mod resources;
pub mod rules;
pub mod snapshots;

use uuid::Uuid;

/// Tenant id. Obtained only from an authenticated [`crate::auth::Principal`] or bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrgId(pub Uuid);

/// User id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserId(pub Uuid);
