//! First-run bootstrap (default organisation, owner with an initial password, admin API key)
//! and the operator's password reset.

use hm_core::apikey::{self, NewApiKey};
use hm_core::password;
use hm_core::secret::Secret;
use sqlx::PgPool;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::auth::Scope;
use crate::repo::{self, OrgId, UserId};

#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error(
        "this instance is already bootstrapped (users exist); create further API keys via the API"
    )]
    AlreadyBootstrapped,
    #[error("invalid input: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Crypto(#[from] hm_core::crypto::CryptoError),
    #[error(transparent)]
    Password(#[from] password::PasswordError),
    #[error("no user with this e-mail address")]
    UnknownUser,
}

/// Result of a successful bootstrap. `key.full` and `password` must be shown to the operator once.
pub struct Bootstrapped {
    pub org_id: Uuid,
    pub key: NewApiKey,
    /// Generated initial password of the owner (log in with it, then change it).
    pub password: Secret<Zeroizing<String>>,
}

/// Creates org, owner and admin key in one transaction — only if no user exists yet.
pub async fn run(db: &PgPool, org_name: &str, email: &str) -> Result<Bootstrapped, BootstrapError> {
    let org_name = org_name.trim();
    let email = email.trim();
    if org_name.is_empty() || org_name.len() > 200 {
        return Err(BootstrapError::Invalid(
            "organisation name must be 1–200 characters",
        ));
    }
    if !email.contains('@') || email.len() > 320 {
        return Err(BootstrapError::Invalid("e-mail address looks invalid"));
    }

    let initial = password::generate()?;
    let hash = password::hash(initial.expose())?;
    let mut tx = db.begin().await?;
    // Serialise concurrent bootstrap attempts.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('hetzner-monitor:bootstrap'))")
        .execute(&mut *tx)
        .await?;
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;
    if users > 0 {
        return Err(BootstrapError::AlreadyBootstrapped);
    }
    let org_id: Uuid =
        sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id")
            .bind(org_name)
            .fetch_one(&mut *tx)
            .await?;
    let user_id: Uuid =
        sqlx::query_scalar("INSERT INTO users (email, password_hash) VALUES ($1, $2) RETURNING id")
            .bind(email)
            .bind(&hash)
            .fetch_one(&mut *tx)
            .await?;
    sqlx::query("INSERT INTO memberships (org_id, user_id, role) VALUES ($1, $2, 'owner')")
        .bind(org_id)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    let key = apikey::generate()?;
    repo::api_keys::insert(
        &mut *tx,
        OrgId(org_id),
        UserId(user_id),
        "bootstrap",
        &[Scope::Admin],
        &key,
    )
    .await?;
    tx.commit().await?;
    Ok(Bootstrapped {
        org_id,
        key,
        password: initial,
    })
}

/// Operator tool: gives the user a new random password and ends all their sessions.
/// Returns the password to show once.
pub async fn reset_password(
    db: &PgPool,
    email: &str,
) -> Result<Secret<Zeroizing<String>>, BootstrapError> {
    let user = repo::users::id_by_email(db, email.trim())
        .await?
        .ok_or(BootstrapError::UnknownUser)?;
    let new = password::generate()?;
    let hash = password::hash(new.expose())?;
    let mut tx = db.begin().await?;
    repo::users::set_password_hash(&mut *tx, UserId(user), &hash).await?;
    repo::sessions::delete_for_user(&mut *tx, UserId(user), None).await?;
    tx.commit().await?;
    Ok(new)
}
