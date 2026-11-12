//! Users and their memberships. Users are global (one e-mail across orgs); memberships tie
//! them to a tenant with a role.

use sqlx::{PgExecutor, PgPool};
use uuid::Uuid;

use super::{OrgId, UserId};

/// Pre-authentication lookup for login: the user by e-mail (case-insensitive), with its
/// password hash and the membership to log into. Returns the tenant.
#[derive(Debug, sqlx::FromRow)]
pub struct LoginCandidate {
    pub user_id: Uuid,
    pub password_hash: Option<String>,
    pub disabled: bool,
    pub org_id: Option<Uuid>,
}

/// Finds the user and a membership: the one in `org` if given, else the oldest.
pub async fn find_for_login(
    db: &PgPool,
    email: &str,
    org: Option<Uuid>,
) -> Result<Option<LoginCandidate>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.id AS user_id, u.password_hash, u.disabled_at IS NOT NULL AS disabled,
                (SELECT m.org_id FROM memberships m
                 WHERE m.user_id = u.id AND ($2::uuid IS NULL OR m.org_id = $2)
                 ORDER BY m.created_at, m.org_id LIMIT 1) AS org_id
         FROM users u WHERE u.email = $1::citext",
    )
    .bind(email)
    .bind(org)
    .fetch_optional(db)
    .await
}

/// Profile as shown by `/me` and login.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct Me {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub org_id: Uuid,
    pub org_name: String,
    pub role: String,
}

/// The caller's profile in its org.
pub async fn me(db: &PgPool, org: OrgId, user: UserId) -> Result<Option<Me>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.id AS user_id, u.email::text AS email, u.name, o.id AS org_id,
                o.name AS org_name, m.role
         FROM memberships m
         JOIN users u ON u.id = m.user_id
         JOIN organizations o ON o.id = m.org_id
         WHERE m.org_id = $1 AND m.user_id = $2",
    )
    .bind(org.0)
    .bind(user.0)
    .fetch_optional(db)
    .await
}

/// The stored password hash of a member of this org.
pub async fn password_hash(
    db: &PgPool,
    org: OrgId,
    user: UserId,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT u.password_hash FROM users u
         JOIN memberships m ON m.user_id = u.id AND m.org_id = $1
         WHERE u.id = $2",
    )
    .bind(org.0)
    .bind(user.0)
    .fetch_optional(db)
    .await
    .map(Option::flatten)
}

/// Sets a user's password hash (PHC string).
pub async fn set_password_hash(
    db: impl PgExecutor<'_>,
    user: UserId,
    hash: &str,
) -> Result<bool, sqlx::Error> {
    Ok(
        sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
            .bind(user.0)
            .bind(hash)
            .execute(db)
            .await?
            .rows_affected()
            == 1,
    )
}

/// CLI lookup by e-mail (operator tool, all tenants).
pub async fn id_by_email(db: &PgPool, email: &str) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM users WHERE email = $1::citext")
        .bind(email)
        .fetch_optional(db)
        .await
}
