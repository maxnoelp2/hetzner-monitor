//! First-run bootstrap: default organisation, owner and an admin API key.

use hm_core::apikey::{self, NewApiKey};
use sqlx::PgPool;
use uuid::Uuid;

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
}

/// Result of a successful bootstrap. `key.full` must be shown to the operator once.
pub struct Bootstrapped {
    pub org_id: Uuid,
    pub key: NewApiKey,
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
    let user_id: Uuid = sqlx::query_scalar("INSERT INTO users (email) VALUES ($1) RETURNING id")
        .bind(email)
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
    Ok(Bootstrapped { org_id, key })
}
