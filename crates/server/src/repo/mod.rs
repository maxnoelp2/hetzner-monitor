//! Database access. **Every function on tenant data takes an [`OrgId`]** and filters by it
//! (ARCHITECTURE.md, invariant 5). The only exceptions are API key lookup by prefix, which
//! happens before the tenant is known and returns the tenant, the scheduler's
//! [`projects::claim_due`], which returns each claimed project with its tenant, and the
//! notifier's [`notifications::claim_next`], which joins channel and alert on the same tenant,
//! the session lookup by token hash and the login lookup by e-mail (both before the tenant is
//! known, both return it), user-wide session and password functions (a password belongs to the
//! user, not to one org), and the operator tools in [`crate::keys`] and the `users` CLI.

pub mod alerts;
pub mod api_keys;
pub mod channels;
pub mod notifications;
pub mod projects;
pub mod resources;
pub mod rules;
pub mod sessions;
pub mod silences;
pub mod snapshots;
pub mod users;

use uuid::Uuid;

/// Tenant id. Obtained only from an authenticated [`crate::auth::Principal`] or bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrgId(pub Uuid);

/// User id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserId(pub Uuid);
