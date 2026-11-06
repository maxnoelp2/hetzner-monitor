//! Database access. **Every function on tenant data takes an [`OrgId`]** and filters by it
//! (ARCHITECTURE.md, invariant 5). The only exception is API key lookup by prefix, which
//! happens before the tenant is known and returns the tenant.

pub mod api_keys;
pub mod projects;
pub mod resources;

use uuid::Uuid;

/// Tenant id. Obtained only from an authenticated [`crate::auth::Principal`] or bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrgId(pub Uuid);

/// User id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserId(pub Uuid);
